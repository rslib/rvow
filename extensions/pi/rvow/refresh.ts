export function refreshLoop(work: () => Promise<unknown>, interval = 5_000): () => void {
  const delay = Math.max(1_000, Math.min(60_000, interval));
  let stopped = false;
  let timer: ReturnType<typeof setTimeout>;
  const schedule = () => {
    if (stopped) return;
    timer = setTimeout(() => {
      void Promise.resolve()
        .then(work)
        .catch(() => undefined)
        .finally(schedule);
    }, delay);
    timer.unref();
  };
  schedule();
  return () => {
    stopped = true;
    clearTimeout(timer);
  };
}
