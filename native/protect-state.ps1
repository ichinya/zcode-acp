param([Parameter(Mandatory = $true)][string]$Directory)
$ErrorActionPreference = 'Stop'
$target = Get-Item -LiteralPath $Directory -Force
if (-not $target.PSIsContainer -or $target.Name -notmatch '^bb-native-[0-9a-f-]{36}$' -or ($target.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
    throw 'Expected a dedicated native ACP session directory'
}
$identity = [Security.Principal.WindowsIdentity]::GetCurrent().User
$acl = [Security.AccessControl.DirectorySecurity]::new()
$acl.SetOwner($identity)
$acl.SetAccessRuleProtection($true, $false)
$rule = [Security.AccessControl.FileSystemAccessRule]::new($identity, [Security.AccessControl.FileSystemRights]::FullControl, [Security.AccessControl.InheritanceFlags]'ContainerInherit,ObjectInherit', [Security.AccessControl.PropagationFlags]::None, [Security.AccessControl.AccessControlType]::Allow)
$acl.AddAccessRule($rule)
Set-Acl -LiteralPath $target.FullName -AclObject $acl
