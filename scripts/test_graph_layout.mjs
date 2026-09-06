// Geometry and fold-state regressions; no browser or third-party dependencies.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
globalThis.React = { memo: (component) => component };
globalThis.window = { ReactFlow: {} };
const source = await readFile(new URL('../frontend/graph.js', import.meta.url), 'utf8');
const { layout, defaultCollapsed, branchState, dragDetached, visibleEntries } = await import(`data:text/javascript;base64,${Buffer.from(source).toString('base64')}`);

const nodes = [
  { id: 'a', parent: 'root' }, { id: 'b', parent: 'root' },
  { id: 'a2', parent: 'a' }, { id: 'a3', parent: 'a2' },
  { id: 'a4', parent: 'a3' }, { id: 'a5', parent: 'a4' },
];
assert.deepEqual([...defaultCollapsed(nodes)].sort(), ['a3', 'a4']);
const overrides = new Map([['a', true], ['a3', false]]);
const merged = [...nodes, { id: 'new', parent: 'a5' }];
const state = branchState(merged, overrides);
assert(state.has('a'), 'Adding an AI child must not reopen a manually closed ancestor');
assert(!state.has('a3'), 'Keep an explicitly opened branch');
assert(state.has('a5'), 'A newly populated deep branch defaults to closed');
assert(state.has('a4'), 'Opening a parent must not recursively open descendants');

const dimensions = new Map([
  ['root', { width: 360, height: 90 }], ['a', { width: 360, height: 550 }],
  ['b', { width: 360, height: 120 }], ['a2', { width: 360, height: 90 }],
]);
const visible = nodes.slice(0, 3);
const positions = layout(visible, 'target', new Set(visible.map((n) => n.id)), new Map(), dimensions);
const centre = id => positions.get(id).x + (dimensions.get(id)?.width || 360) / 2;
assert.equal(centre('root'), 0, 'Root is centred over the row');
assert.equal(centre('a'), centre('a2'), 'Single-child branch is horizontally centred');
assert.equal(positions.get('a').y, positions.get('b').y, 'Siblings occupy one row');
assert(positions.get('a').x + dimensions.get('a').width + 24 <= positions.get('b').x, 'Sibling subtrees do not overlap');
for (const n of visible) assert(positions.get(n.id).y >= positions.get(n.parent).y + dimensions.get(n.parent).height + 60, 'Children stay below the measured parent');
assert.equal((positions.get('a').x+positions.get('b').x+dimensions.get('b').width)/2,0);
const detached = layout([...visible, { id: 'orphan', parent: null }, { id: 'child', parent: 'orphan' }], 'target');
assert(detached.has('orphan') && detached.get('child').y > detached.get('orphan').y);
assert.deepEqual(layout(visible.map(n=>({...n,position:[9999,9999]})),'target'),layout(visible,'target'),'Connected nodes ignore legacy drag coordinates');
// Mixed-height cards and nested rows must reserve full subtree width, on every graph surface.
const wide=[...visible,{id:'a2b',parent:'a'},{id:'b2',parent:'b'},{id:'b3',parent:'b2'},{id:'b4',parent:'b3'}];
const measured=new Map(wide.map((n,i)=>[n.id,{width:320+i*11,height:120+i*67}]));
measured.set('root',{width:360,height:100});
for(const type of ['target','actual']) {
 const pos=layout(wide,type,new Set(),new Map(),measured);
 const all=[{id:'root'},...wide];
 for(let i=0;i<all.length;i++) for(let j=i+1;j<all.length;j++) {
  const a=pos.get(all[i].id),b=pos.get(all[j].id),sa=measured.get(all[i].id),sb=measured.get(all[j].id);
  assert(a.x+sa.width<=b.x || b.x+sb.width<=a.x || a.y+sa.height<=b.y || b.y+sb.height<=a.y, `No card overlap: ${type} ${all[i].id}/${all[j].id}`);
 }
}
console.log('PASS compact rows, measured dimensions, detached branches and unchanged depth folding');
const forest = [...visible, {id:'free',parent:null,position:[-120,500]}, {id:'child',parent:'free'}, {id:'leaf',parent:'child'}];
const original = layout(forest, 'target');
assert.deepEqual(original.get('free'), {x:-120,y:500});
const graph = [...original].map(([id,position])=>({id,position}));
const dragged = dragDetached(graph, forest, [{type:'position',id:'free',position:{x:280,y:650}}]);
const moved = new Map(dragged.map(n=>[n.id,n.position]));
for(const id of ['free','child','leaf']) assert.deepEqual(moved.get(id), {x:original.get(id).x+400,y:original.get(id).y+150});
assert.deepEqual(moved.get('a'), original.get('a'), 'Dragging a detached tree cannot move connected branches');
assert.deepEqual(dragDetached(graph,forest,[{type:'position',id:'child',position:{x:0,y:0}}]),graph,'Children cannot be independently dragged');
const saved = forest.map(n=>n.id==='free'?{...n,position:[280,650]}:n);
assert.deepEqual(layout(saved,'target'),moved,'Saved anchor preserves all relative subtree positions after reload');
const reattached=saved.map(n=>n.id==='free'?{...n,parent:'a'}:n);
assert.deepEqual(layout(reattached,'target'),layout(reattached.map(n=>({...n,position:null})),'target'),'Reconnection restores automatic layout');
assert.deepEqual(dragDetached(graph,forest,[{type:'position',id:'free',position:{x:Infinity,y:0}}]),graph);
console.log('PASS detached subtree dragging, persistence, connected locks and reattachment');

const entries = [{kind:"file",extension:"URL"},{kind:"file",extension:"lnk"},{kind:"file",extension:"txt"},{kind:"directory",extension:"url"}];
assert.equal(visibleEntries({mode:"desktop",entries}).length,2);
assert.equal(visibleEntries({mode:"organize",entries}).length,4);
console.log("PASS desktop shortcut visibility without altering download listings or folders");
