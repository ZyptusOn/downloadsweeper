import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
const source = await readFile(new URL('../frontend/proposal_selection.js', import.meta.url), 'utf8');
const {proposalDependencies, validProposalSelection} = await import(`data:text/javascript;base64,${Buffer.from(source).toString('base64')}`);
const add = (id, parent) => ({id, target:id, kind:'node', before:null, after:{parent,extensions:[]}});
const changes = [add('grandchild','child'), add('child','parent'), add('parent','root'), add('independent','root')];
const deps = proposalDependencies(changes);
let selected = validProposalSelection(new Set(['grandchild','child','independent']),deps);
assert.deepEqual([...selected],['independent'], 'Rejecting a parent disables the entire dependent subtree');
selected.add('grandchild');
assert(!validProposalSelection(selected,deps).has('grandchild'), 'A child cannot silently re-enable a rejected parent');
selected.add('parent'); selected = validProposalSelection(selected,deps);
assert(!selected.has('child'), 'Reaccepting a parent does not auto-accept its children');
selected.add('child'); selected.add('grandchild');
assert.equal(validProposalSelection(selected,deps).size,4);
const old = {id:'old',parent:'root',extensions:['mp3','zip']};
const split = [{id:'release',target:'old',kind:'node',before:old,after:{...old,extensions:['zip']}},
  {...add('music','root'),after:{parent:'root',extensions:['mp3']}}];
assert.deepEqual([...proposalDependencies(split,[old]).get('music')],['release']);
assert.equal(validProposalSelection(new Set(['music']),proposalDependencies(split,[old])).size,0);
console.log('PASS proposal parent/descendant dependencies, independent choices, explicit re-selection and extension reassignment');

const placement={id:"place",kind:"placement",target:"file",after:{node_id:"child"}};
assert(!validProposalSelection(new Set(["place"]),proposalDependencies([...changes,placement])).has("place"));
