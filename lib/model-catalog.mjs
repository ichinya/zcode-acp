// Source identity is part of the native ref, independent of a model's label.
export const nativeModelId = ref => Buffer.from(JSON.stringify({ providerId: ref.providerId, modelId: ref.modelId })).toString("base64url");

function source(model) {
  const id = model.ref.providerId;
  const account = /^account:(zai|bigmodel)-(individual-coding-plan|team-coding-plan|start-plan|offpeak-idle-plan|highspeed-card)$/.exec(id);
  if (account) {
    const plans = {
      "individual-coding-plan": [0, "Coding Plan"], "start-plan": [1, "Start Plan"],
      "team-coding-plan": [2, "Team Coding Plan"], "offpeak-idle-plan": [3, "Off-peak Plan"],
      "highspeed-card": [4, "Highspeed Card"],
    };
    const [rank, plan] = plans[account[2]];
    return { id, rank, name: `${plan} · ${account[1] === "zai" ? "Z.ai" : "BigModel"}` };
  }
  return { id, rank: 10, name: model.providerLabel?.trim() || id };
}

export function modelCatalog(available) {
  const sources = new Map();
  for (const model of available) {
    if (model.disabledReason) continue;
    const group = source(model);
    if (!sources.has(group.id)) sources.set(group.id, { ...group, models: [] });
    sources.get(group.id).models.push(model);
  }
  const groups = [...sources.values()].sort((a, b) => a.rank - b.rank);
  const names = new Map();
  for (const group of groups) names.set(group.name, (names.get(group.name) || 0) + 1);
  const models = [], options = [];
  for (const group of groups) {
    // Two personal connections can have the same display label. Keep them
    // visibly distinct as well as independently routable.
    const name = names.get(group.name) > 1 ? `${group.name} (${group.id})` : group.name;
    const choices = group.models.map(model => {
      const id = nativeModelId(model.ref);
      // BB 0.49 flattens model option groups, so retain the source in each row.
      const label = `${name} / ${model.label || model.ref.modelId}`;
      models.push({ modelId: id, name: label, description: `Source: ${name}` });
      return { value: id, name: label, description: `Source: ${name}` };
    });
    options.push({ group: group.id, name, options: choices });
  }
  return { models, options };
}
