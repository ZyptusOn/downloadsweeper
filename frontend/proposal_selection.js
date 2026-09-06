// Dependencies refer to proposed edits, not disk paths. Selection never enables another edit.
export function proposalDependencies(changes, nodes = []) {
  const byTarget = new Map(changes.filter(c => c.kind === 'node').map(c => [c.target, c]));
  const result = new Map(changes.map(c => [c.id, new Set()]));
  for (const change of changes) {
    if (change.kind === 'placement') {
      const destination = byTarget.get(change.after?.node_id);
      if (destination?.after) result.get(change.id).add(destination.id);
    }
    if (change.kind !== 'node' || !change.after) continue;
    const parent = byTarget.get(change.after.parent);
    if (parent?.after && (!parent.before || parent.before.parent !== parent.after.parent))
      result.get(change.id).add(parent.id);
    // Splitting a format out of an existing top-level category also requires releasing its old rule.
    if (change.after.parent === 'root') for (const ext of change.after.extensions || []) {
      if (ext === '*') continue;
      for (const node of nodes.filter(n => n.id !== change.target && n.parent === 'root'
        && n.extensions?.some(e => e.toLowerCase() === ext.toLowerCase()))) {
        const release = byTarget.get(node.id);
        if (release && (!release.after || !(release.after.extensions || []).some(e => e.toLowerCase() === ext.toLowerCase())))
          result.get(change.id).add(release.id);
      }
    }
  }
  return result;
}

export function validProposalSelection(selected, dependencies) {
  const result = new Set(selected);
  let changed = true;
  while (changed) {
    changed = false;
    for (const id of result) if ([...(dependencies.get(id) || [])].some(dep => !result.has(dep))) {
      result.delete(id); changed = true;
    }
  }
  return result;
}
