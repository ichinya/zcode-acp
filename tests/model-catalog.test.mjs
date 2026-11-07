import assert from "node:assert/strict";
import test from "node:test";
import { modelCatalog, nativeModelId } from "../lib/model-catalog.mjs";

const model = (providerId, providerLabel, modelId = "shared-model", extra = {}) => ({ ref: { providerId, modelId }, label: modelId, providerLabel, ...extra });

test("identical model names remain distinct across plans and personal providers, including flattened ACP clients", () => {
  const available = [model("personal", "My API"), model("account:zai-start-plan", "Z.AI Start Plan"), model("account:bigmodel-individual-coding-plan", "BigModel"), model("account:zai-individual-coding-plan", "Z.AI Individual Coding Plan")];
  const result = modelCatalog(available);
  assert.deepEqual(result.options.map(g => g.name), ["Coding Plan · BigModel", "Coding Plan · Z.ai", "Start Plan · Z.ai", "My API"]);
  assert.equal(new Set(result.models.map(m => m.name)).size, 4);
  assert.equal(new Set(result.models.map(m => m.modelId)).size, 4);
  assert.deepEqual(result.options.flatMap(g => g.options.map(o => [o.value, o.name])), result.models.map(m => [m.modelId, m.name]));
  for (const m of available) assert.ok(result.models.some(row => row.modelId === nativeModelId(m.ref)));
  assert.equal(nativeModelId({ ...available[0].ref, options: { reasoningLevel: "max" } }), nativeModelId(available[0].ref));
});

test("personal provider labels never infer a plan, and duplicate provider names remain distinguishable", () => {
  const result = modelCatalog([model("one", "Start Plan"), model("two", "Start Plan"), model("unknown", ""), model("account:future", "Future account")]);
  assert.deepEqual(result.options.map(g => g.group), ["one", "two", "unknown", "account:future"]);
  assert.deepEqual(result.options.map(g => g.name), ["Start Plan (one)", "Start Plan (two)", "unknown", "Future account"]);
  assert.equal(new Set(result.models.map(m => m.name)).size, 4);
});

test("only available native sources appear; disabled Start Plan is not advertised", () => {
  const result = modelCatalog([model("account:zai-individual-coding-plan", "Z.ai"), model("account:zai-start-plan", "Z.ai", "disabled", { disabledReason: "No access" }), model("api", "API", "first"), model("api", "API", "second")]);
  assert.deepEqual(result.options.map(g => g.name), ["Coding Plan · Z.ai", "API"]);
  assert.deepEqual(result.options[1].options.map(o => o.name), ["API / first", "API / second"]);
  assert.deepEqual(modelCatalog([]), { models: [], options: [] });
});
