// Only confirmed batch completions contribute to progress; active workers are not estimates.
export function parallelSummary(run) {
  const batches = run?.batches || [];
  const active = batches.filter(b => b.status === 'running');
  const done = batches.filter(b => b.status === 'complete');
  const queued = batches.filter(b => !['running', 'complete'].includes(b.status));
  const total = batches.reduce((n, b) => n + b.files, 0);
  const completed = done.reduce((n, b) => n + b.files, 0);
  return {active, done, queued, total, completed, percent: total ? Math.min(100, 100 * completed / total) : 0};
}
