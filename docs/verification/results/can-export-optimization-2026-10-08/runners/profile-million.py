import argparse, hashlib, json, os, resource, shutil, signal, subprocess, sys, time
from pathlib import Path
sys.path.insert(0, '/home/hideki/DIR-Simulator/scripts/performance')
from measure_can_million import GIB, available_memory, child_resources, digest, save, utc
a=argparse.ArgumentParser();a.add_argument('--binary',type=Path,required=True);a.add_argument('--seconds',type=int,default=60);args=a.parse_args()
root=Path('/tmp/dir-profile-can-2026-10-08');d=root/'actualmillion-profile';d.mkdir()
binary=args.binary.resolve(strict=True);config=Path('/home/hideki/DIR-Simulator/docs/verification/fixtures/performance/can-million/rho-0.30.ini');output=d/'output';perf=d/'perf.data'
command=['/usr/lib/linux-tools/6.8.0-142-generic/perf','record','-F','199','-e','cpu-clock:u','--call-graph','dwarf,16384','-o',str(perf),'--',str(binary),'run','--config',str(config),'--output',str(output)]
report={'started_at_utc':utc(),'command':command,'binary_sha256':digest(binary),'config_sha256':digest(config),'profiling_window_seconds':args.seconds,'qualification':'Actual million-request input, bounded CPU sampling prefix only; not a formal performance acceptance run.'}
save(d/'profile.json',report)
def guard():
 resource.setrlimit(resource.RLIMIT_AS,(26*GIB,26*GIB));resource.setrlimit(resource.RLIMIT_CORE,(1,1))
started=time.monotonic();reason=None;peak=0
with (d/'stdout.json').open('w') as out,(d/'stderr.txt').open('w') as err,(d/'samples.jsonl').open('w') as samples:
 process=subprocess.Popen(command,stdout=out,stderr=err,start_new_session=True,preexec_fn=guard)
 try:
  while process.poll() is None:
   elapsed=time.monotonic()-started;r=child_resources(process.pid);available=available_memory()
   if r:peak=max(peak,r.get('VmRSS',0))
   samples.write(json.dumps({'elapsed_seconds':round(elapsed,3),'child':r,'available_memory_bytes':available})+'\n');samples.flush()
   if reason is None and ((r and r.get('VmRSS',0)>24*GIB) or available<3*GIB):reason='host_memory_guard'
   if reason is None and elapsed>=args.seconds:reason='profiling_window_limit'
   if reason:
    os.killpg(process.pid,signal.SIGTERM)
    try:process.wait(timeout=10)
    except subprocess.TimeoutExpired:os.killpg(process.pid,signal.SIGKILL);process.wait()
    break
   time.sleep(.25)
 except BaseException:
  if process.poll() is None:os.killpg(process.pid,signal.SIGTERM);process.wait()
  raise
report.update(finished_at_utc=utc(),returncode=process.returncode,stop_reason=reason,wall_observed_seconds=time.monotonic()-started,maximum_sampled_rss_bytes=peak,completed=(output/'manifest.json').exists(),unpublished_files=[{'path':str(p.relative_to(output)),'bytes':p.stat().st_size} for p in output.rglob('*') if p.is_file()])
if perf.exists():report['perf_data']={'path':str(perf),'bytes':perf.stat().st_size,'sha256':digest(perf)}
save(d/'profile.json',report)
if output.exists():shutil.rmtree(output);report['unpublished_outputs_removed_after_inventory']=True;save(d/'profile.json',report)
print(json.dumps({k:v for k,v in report.items() if k not in ['unpublished_files','command']}),flush=True)
