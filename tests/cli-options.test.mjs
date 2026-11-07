import assert from "node:assert/strict";
import test from "node:test";
import { cliEnvironment } from "../lib/cli-options.mjs";

test("native entry CLI options preserve spaces and override the environment without mutating it", () => {
  const env = { ZCODE_NATIVE_ENTRY: "old", KEEP: "unchanged" };
  const entry = "D:\\Custom ZCode\\build=release\\resources\\glm\\zcode.cjs";
  for (const args of [["--native-entry", entry], [`--native-entry=${entry}`]]) {
    assert.deepEqual(cliEnvironment(args, env), { ...env, ZCODE_NATIVE_ENTRY: entry });
    assert.equal(env.ZCODE_NATIVE_ENTRY, "old");
  }
  assert.deepEqual(cliEnvironment([], env), env);
  assert.deepEqual(cliEnvironment([], {}), {});
});

test("invalid or ambiguous native entry arguments fail rather than reverting to discovery", () => {
  for (const args of [["--native-entry"], ["--native-entry", "--doctor"], ["--native-entry="], ["--native-entry", "   "], ["--native-entry=x", "--native-entry=y"], ["--native-entry=x\0y"]]) {
    assert.throws(() => cliEnvironment(args, {}), /--native-entry/);
  }
});
