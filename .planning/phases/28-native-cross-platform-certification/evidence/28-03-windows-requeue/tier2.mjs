// Mirrors f28-native-soak.mjs runSession() EXACTLY: spawn with stdio pipes, close stdin
// immediately, collect stdout/stderr, 60s cap. Any difference from the real harness would
// make this probe measure something other than what the soak will measure.
import { spawn } from 'node:child_process';
const bin = process.argv[2];
const argv = process.argv.slice(3);
const started = Date.now();
let out = Buffer.alloc(0), err = Buffer.alloc(0), done = false;
const child = spawn(bin, argv, { windowsHide: true, stdio: ['pipe','pipe','pipe'] });
const timer = setTimeout(() => { if (!done) { try { child.kill('SIGKILL'); } catch {} } }, 60000);
child.stdout.on('data', d => { out = Buffer.concat([out, d]); });
child.stderr.on('data', d => { err = Buffer.concat([err, d]); });
try { child.stdin.end(); } catch {}
child.on('close', (status, signal) => {
  done = true; clearTimeout(timer);
  console.log(`ARGV=${argv.join(' ')}`);
  console.log(`STATUS=${status} SIGNAL=${signal}`);
  console.log(`MS=${Date.now()-started}`);
  console.log(`STDOUT_BYTES=${out.length} STDERR_BYTES=${err.length}`);
  console.log(`STDOUT_HEAD=${JSON.stringify(out.toString('utf8').slice(0,300))}`);
  console.log(`STDERR_HEAD=${JSON.stringify(err.toString('utf8').slice(0,300))}`);
});
