// Read-only compatibility with ZCode's credential cipher. Never migrate, create
// or update Desktop credentials, OS entries, source markers or secret files.
use crate::common::*;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::Aead};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};
use std::{io::Read, path::Path};

fn unavailable() -> Error {
    Error::new(
        "Cannot read ZCode credential encryption key. Use the same OS user, unlock the OS credential store, and use the same credential secret as ZCode",
    )
}
fn invalid() -> Error {
    Error::new("Invalid or unsupported encrypted ZCode credential")
}
fn digest(value: impl AsRef<[u8]>) -> [u8; 32] {
    Sha256::digest(value.as_ref()).into()
}
fn fingerprint(key: &[u8; 32]) -> String {
    hash(key)[..24].into()
}
fn open(value: &str, key: &[u8; 32]) -> Result<String> {
    let fields: Vec<_> = value.split('.').collect();
    if fields.len() != 3 {
        return Err(invalid());
    }
    let decode = |s| URL_SAFE_NO_PAD.decode(s).map_err(|_| invalid());
    let iv = decode(fields[0])?;
    let tag = decode(fields[1])?;
    let mut bytes = decode(fields[2])?;
    if iv.len() != 12 || tag.len() != 16 {
        return Err(invalid());
    }
    bytes.extend(tag);
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| invalid())?;
    let plain = cipher
        .decrypt(Nonce::from_slice(&iv), bytes.as_ref())
        .map_err(|_| invalid())?;
    String::from_utf8(plain).map_err(|_| invalid())
}
pub struct Reader<'a> {
    file: &'a Path,
    env: &'a Env,
    os_key: Option<[u8; 32]>,
}
impl<'a> Reader<'a> {
    pub fn new(file: &'a Path, env: &'a Env) -> Self {
        Self {
            file,
            env,
            os_key: None,
        }
    }
    fn managed_key(&self, allow_private_file: bool) -> Result<Option<[u8; 32]>> {
        let secret = self
            .env
            .get("ZCODE_CREDENTIAL_SECRET")
            .filter(|s| !s.is_empty());
        let secret_file = self
            .env
            .get("ZCODE_CREDENTIAL_SECRET_FILE")
            .filter(|s| !s.is_empty());
        if secret.is_some() && secret_file.is_some() {
            return Err(unavailable());
        }
        if let Some(secret) = secret {
            return Ok(Some(digest(secret)));
        }
        let parent = self.file.parent().ok_or_else(unavailable)?;
        let file = match secret_file {
            Some(file) => std::path::PathBuf::from(file),
            None if allow_private_file => {
                match std::fs::read_to_string(parent.join("credential-source")) {
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    _ => return Err(unavailable()),
                }
                parent.join("credential-secret")
            }
            None => return Ok(None),
        };
        let mut f = match std::fs::File::open(&file) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && secret_file.is_none() => {
                return Ok(None);
            }
            Err(_) => return Err(unavailable()),
        };
        let metadata = f.metadata().map_err(|_| unavailable())?;
        if !metadata.is_file() {
            return Err(unavailable());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if secret_file.is_none() && metadata.permissions().mode() & 0o077 != 0 {
                return Err(unavailable());
            }
        }
        let mut secret = Vec::new();
        Read::by_ref(&mut f)
            .take(4097)
            .read_to_end(&mut secret)
            .map_err(|_| unavailable())?;
        if !(32..=4096).contains(&secret.len()) || std::str::from_utf8(&secret).is_err() {
            return Err(unavailable());
        }
        Ok(Some(digest(secret)))
    }
    pub fn decrypt(&mut self, value: &str) -> Result<String> {
        if let Some(sealed) = value.strip_prefix("enc:v1:") {
            let fallback = format!(
                "zcode-credential-fallback:{}:{}:{}",
                platform(),
                path_string(home()),
                whoami::username()
            );
            let secret = self
                .env
                .get("ZCODE_CREDENTIAL_SECRET")
                .filter(|s| !s.is_empty());
            if secret.is_some()
                && self
                    .env
                    .get("ZCODE_CREDENTIAL_SECRET_FILE")
                    .is_some_and(|s| !s.is_empty())
            {
                return Err(unavailable());
            }
            if let Some(secret) = secret {
                for candidate in [secret.as_str(), secret.trim(), fallback.as_str()] {
                    if let Ok(plain) = open(sealed, &digest(candidate)) {
                        return Ok(plain);
                    }
                }
                return Err(invalid());
            }
            if let Some(key) = self.managed_key(false)?
                && let Ok(plain) = open(sealed, &key)
            {
                return Ok(plain);
            }
            return open(sealed, &digest(fallback));
        }
        let Some(sealed) = value.strip_prefix("enc:v2:") else {
            return if value.starts_with("enc:") {
                Err(invalid())
            } else {
                Ok(value.into())
            };
        };
        let fields: Vec<_> = sealed.split(':').collect();
        if fields.len() != 3
            || fields[1].len() != 24
            || !fields[1]
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        {
            return Err(invalid());
        }
        let configured = self.managed_key(false)?;
        let key = match fields[0] {
            "env" => configured
                .or(self.managed_key(true)?)
                .ok_or_else(unavailable)?,
            "os" if configured.is_none() => {
                if self.os_key.is_none() {
                    self.os_key = Some(read_os_key(self.file)?);
                }
                self.os_key.unwrap()
            }
            _ => return Err(unavailable()),
        };
        if fingerprint(&key) != fields[1] {
            return Err(unavailable());
        }
        open(fields[2], &key)
    }
}
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
fn read_os_key(file: &Path) -> Result<[u8; 32]> {
    let account = hash(path_string(absolute(file)?));
    let encoded = keyring::Entry::new("ZCode Credential Encryption", &account)
        .and_then(|entry| entry.get_password())
        .map_err(|_| unavailable())?;
    if encoded.len() != 43 {
        return Err(unavailable());
    }
    URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| unavailable())?
        .try_into()
        .map_err(|_| unavailable())
}
#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn read_os_key(_file: &Path) -> Result<[u8; 32]> {
    Err(unavailable())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn seal(value: &str, source: &str, key: &[u8; 32]) -> String {
        let cipher = Aes256Gcm::new_from_slice(key).unwrap();
        let iv = [8_u8; 12];
        let mut bytes = cipher
            .encrypt(Nonce::from_slice(&iv), value.as_bytes())
            .unwrap();
        let tag = bytes.split_off(bytes.len() - 16);
        format!(
            "enc:v2:{source}:{}:{}.{}.{}",
            fingerprint(key),
            URL_SAFE_NO_PAD.encode(iv),
            URL_SAFE_NO_PAD.encode(tag),
            URL_SAFE_NO_PAD.encode(bytes)
        )
    }
    #[test]
    fn v2_os_and_managed_secrets_are_read_only_and_fail_closed() {
        let root = std::env::temp_dir().join(format!("zcode-v2-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("credentials.json");
        let key = [42_u8; 32];
        let env = Env::new();
        let mut reader = Reader::new(&file, &env);
        // Inject the OS key without creating a real credential-store entry.
        reader.os_key = Some(key);
        let sealed = seal("zai", "os", &key);
        assert_eq!(reader.decrypt(&sealed).unwrap(), "zai");
        assert!(reader.decrypt(&seal("secret", "os", &[7; 32])).is_err());
        assert!(reader.decrypt("enc:v3:unsupported").is_err());
        assert!(reader.decrypt("enc:v2:os:bad:payload").is_err());
        let mut managed = Env::new();
        managed.insert(
            "ZCODE_CREDENTIAL_SECRET".into(),
            "managed-fixture-secret".into(),
        );
        let mut reader = Reader::new(&file, &managed);
        let encoded = seal("jwt", "env", &digest(b"managed-fixture-secret"));
        assert_eq!(reader.decrypt(&encoded).unwrap(), "jwt");
        assert!(reader.decrypt(&sealed).is_err());
        assert!(std::fs::read_dir(&root).unwrap().next().is_none());
        managed.remove("ZCODE_CREDENTIAL_SECRET");
        managed.insert(
            "ZCODE_CREDENTIAL_SECRET_FILE".into(),
            path_string(root.join("fixture-secret")),
        );
        std::fs::write(root.join("fixture-secret"), "x".repeat(32)).unwrap();
        let mut reader = Reader::new(&file, &managed);
        assert_eq!(
            reader
                .decrypt(&seal("jwt", "env", &digest("x".repeat(32))))
                .unwrap(),
            "jwt"
        );
        std::fs::remove_file(root.join("fixture-secret")).unwrap();
        assert!(std::fs::read_dir(&root).unwrap().next().is_none());
        std::fs::remove_dir(root).unwrap();
    }
}
