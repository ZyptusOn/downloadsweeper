import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
const src=await readFile(new URL('../frontend/parallel_progress.js',import.meta.url),'utf8');
const {parallelSummary}=await import(`data:text/javascript;base64,${Buffer.from(src).toString('base64')}`);
const s=parallelSummary({batches:[{status:'complete',files:2},{status:'running',files:8},{status:'running',files:4},{status:'pending',files:6}]});
assert.equal(s.percent,10); assert.equal(s.active.length,2); assert.equal(s.queued.length,1); assert.equal(s.completed,2);
assert.equal(parallelSummary({batches:[]}).percent,0);
assert.equal(parallelSummary({batches:[{status:'complete',files:10}]}).percent,100);
console.log('PASS partial workers never count as completed; correct concurrent, queued and empty states');

// The live Job omits private decisions. Rendering completed live batches must remain safe.
const {runInNewContext}=await import('node:vm');
const app=await readFile(new URL('../frontend/app.js',import.meta.url),'utf8');
const component=app.slice(app.indexOf('function ClassificationProgress('),app.indexOf('function InspectionProgress('));
const render=runInNewContext(component+'; ClassificationProgress', {h:(type,props,...children)=>({type,props,children}),parallelSummary,ParallelProgress:()=>null});
assert.doesNotThrow(()=>render({classification:{status:'running',total:10,batches:[]},job:{kind:'plan_ai',parallel:{total_files:10,completed_files:2,batches:[{id:'b1',branch:'video',files:2,status:'complete'},{id:'b2',branch:'video',files:8,status:'running'}]}}}));
console.log('PASS live completed batch renders without private decision payload');
