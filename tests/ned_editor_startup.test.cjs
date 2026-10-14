'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const {spawn} = require('node:child_process');
const {once} = require('node:events');
const {waitForStartup, redactStartup, stopNativeChild} = require('./ned_editor_startup.cjs');
const secret = 'private-startup-credential';
const frame = JSON.stringify({schema_version:1,status:'listening',url:'http://127.0.0.1:1234/#session='+secret});

async function observe(script, options = {}) {
  const child = spawn(process.execPath, ['-e', script], {stdio:['ignore','pipe','pipe']});
  const closed = once(child, 'close');
  const attempt = {}, reports = [];
  let value, error;
  try { value = await waitForStartup(child, {attempt, report:r=>reports.push(r), ...options}); }
  catch (e) { error = e; }
  finally { await stopNativeChild(child, closed); }
  assert.ok(Number.isInteger(attempt.pid));
  assert.ok(Number.isInteger(attempt.elapsed_ms));
  assert.equal(reports.length, 1, 'settle once');
  assert.ok(!JSON.stringify(reports).includes(secret), 'diagnostics exclude unparsed credentials');
  assert.ok(attempt.exit_code !== null || attempt.signal !== null, 'owned child reaped');
  return {value,error,attempt,reports};
}
test('startup readiness retains original 20-second bound and redacts fresh credentials', async()=>{
  const r=await observe(`process.stderr.write('#session=${secret}');process.stdout.write(${JSON.stringify(frame+'\n')});setInterval(()=>{},1000)`);
  assert.equal(r.attempt.timeout_ms,20000);
  assert.equal(r.attempt.state,'stdout_ready');
  assert.equal(r.value.url,'http://127.0.0.1:1234/#session='+secret);
  assert.match(r.reports[0].stderr,/redacted/);
});
test('early exit records status and sanitized stderr', async()=>{
  const r=await observe(`process.stderr.write('#session=${secret}');process.exit(7)`);
  assert.match(r.error.message,/stopped before startup/);
  assert.equal(r.attempt.state,'exited_before_startup');
  assert.equal(r.attempt.exit_code,7);
});
test('malformed complete frame fails rather than accepting readiness', async()=>{
  const r=await observe(`process.stdout.write('bad #session=${secret}\\n');setInterval(()=>{},1000)`);
  assert.match(r.error.message,/invalid startup frame/);
  assert.equal(r.attempt.state,'invalid_startup_frame');
});
test('bounded timeout preserves partial stdout and reaps the owned child', async()=>{
  const r=await observe(`process.stdout.write(${JSON.stringify(frame)});setInterval(()=>{},1000)`,{timeoutMs:500});
  assert.match(r.error.message,/startup timeout/);
  assert.equal(r.attempt.state,'startup_timeout');
  assert.match(r.reports[0].stdout,/redacted/);
  assert.equal(r.attempt.signal,'SIGTERM');
});
test('Viewer capability paths are sanitized independently of parsed session state',()=>{
  const value=redactStartup('http://127.0.0.1:1234/'+ 'a'.repeat(64)+' #session='+secret);
  assert.ok(!value.includes(secret));assert.ok(!value.includes('a'.repeat(64)));
});
test('cleanup escalates only the owned unresponsive child after bounded grace', async()=>{
  const child=spawn(process.execPath,['-e',"process.on('SIGTERM',()=>{});process.stdout.write('ready\\n');setInterval(()=>{},1000)"],{stdio:['ignore','pipe','pipe']});
  const closed=once(child,'close'); await once(child.stdout,'data');
  await stopNativeChild(child,closed,50);
  assert.equal(child.signalCode,'SIGKILL');
});
