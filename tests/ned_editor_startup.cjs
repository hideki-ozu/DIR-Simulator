'use strict';
const redactStartup = text => String(text)
  .replace(/(http:\/\/127\.0\.0\.1:\d+)\/[a-f0-9]{64}/gi, '$1/[redacted]')
  .replace(/#session=[^\s"']+/gi, '#session=[redacted]');

function waitForStartup(child, {timeoutMs = 20000, attempt = {}, sanitize = redactStartup, report = () => {}} = {}) {
  const started = performance.now();
  let out = '', settled = false;
  Object.assign(attempt, {timeout_ms: timeoutMs, pid: child.pid ?? null, state: 'waiting_stdout', stdout: '', stderr: '', exit_code: null, signal: null});
  return new Promise((resolve, reject) => {
    const finish = (state, value, error) => {
      if (settled) return;
      settled = true; clearTimeout(timer);
      attempt.state = state; attempt.elapsed_ms = Math.round(performance.now() - started);
      report({...attempt, stdout: sanitize(attempt.stdout), stderr: sanitize(attempt.stderr)});
      error ? reject(new Error(error)) : resolve(value);
    };
    const timer = setTimeout(() => finish('startup_timeout', null, 'native editor startup timeout'), timeoutMs);
    child.once('error', () => finish('launch_failed', null, 'native editor launch failed'));
    child.once('close', (code, signal) => {
      attempt.exit_code = code; attempt.signal = signal; attempt.closed_ms = Math.round(performance.now() - started);
      finish('exited_before_startup', null, 'native editor stopped before startup');
    });
    child.stderr.on('data', bytes => { attempt.stderr = (attempt.stderr + bytes).slice(-8192); });
    child.stdout.on('data', bytes => {
      out += bytes; attempt.stdout = out.slice(-8192);
      const end = out.indexOf('\n');
      if (end < 0 || settled) return;
      try { finish('stdout_ready', JSON.parse(out.slice(0, end))); }
      catch { finish('invalid_startup_frame', null, 'invalid startup frame'); }
    });
  });
}
async function stopNativeChild(child, closed, graceMs = 10000) {
  let timer;
  try {
    if (child && child.exitCode === null && child.signalCode === null) {
      child.kill('SIGTERM');
      timer = setTimeout(() => child.kill('SIGKILL'), graceMs);
    }
    await closed?.catch(() => {});
  } finally { clearTimeout(timer); }
}
module.exports = {waitForStartup, redactStartup, stopNativeChild};
