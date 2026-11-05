import re, os, pathlib, tempfile, subprocess, time, json, select, termios, fcntl, struct, signal
binary=os.environ.get('PANELINK_TRACE_CHECK_BINARY',str(pathlib.Path('target/debug/panelink').resolve()))
results=[]
for enabled in [False,True]:
 root=pathlib.Path(tempfile.mkdtemp(prefix='panelink-trace-pty-',dir='/private/tmp')); os.chmod(root,0o700)
 runtime=root/'runtime'; runtime.mkdir(mode=0o700); ti=root/'terminfo';ti.mkdir(mode=0o700)
 subprocess.run(['/usr/bin/tic','-x','-o',str(ti),'docs/terminfo/panelink-probe.terminfo'],check=True,capture_output=True)
 env={'PATH':'/usr/bin:/bin','HOME':str(root),'TERM':'xterm-256color','ZDOTDIR':str(root)}
 if enabled: env['PANELINK_TRACE_DIR']=str(root/'trace')
 server=subprocess.Popen([binary,'serve','--directory',str(runtime),'--terminfo',str(ti),'--initialize','--allow-input','--','/bin/sh','-c','printf TRACE_READY; exec /bin/cat'],env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
 client=None;master=None
 try:
  assert select.select([server.stdout],[],[],10)[0], 'server startup deadline'
  startup=json.loads(server.stdout.readline());assert startup.get('state')=='running',startup
  master,slave=os.openpty();fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',24,100,0,0))
  def session():
   os.setsid();fcntl.ioctl(0,termios.TIOCSCTTY,0)
  started=time.monotonic();client=subprocess.Popen([binary,'attach','--directory',str(runtime),'--context-dir',str(root/'contexts')],stdin=slave,stdout=slave,stderr=slave,preexec_fn=session,env=env)
  os.close(slave);buf=b''
  def until(marker,timeout=10):
   global buf
   deadline=time.monotonic()+timeout
   while marker not in re.sub(rb"\x1b\[[0-?]*[ -/]*[@-~]", b"", buf):
    assert time.monotonic()<deadline, ('client marker deadline',marker,buf[-300:])
    if select.select([master],[],[],0.1)[0]:buf+=os.read(master,65536)
  until(b'TRACE_READY');attach_ms=(time.monotonic()-started)*1000
  times=[]
  for i in range(8):
   marker=('CANARY_INPUT_%02d'%i).encode();buf=b'';t=time.monotonic();os.write(master,marker+b'\r');until(marker);times.append((time.monotonic()-t)*1000)
  os.write(master,b'\x18')
  deadline=time.monotonic()+5
  while client.poll() is None and time.monotonic()<deadline:
   if select.select([master],[],[],0.05)[0]:
    try: os.read(master,65536)
    except OSError: break
  client.wait(timeout=1);assert client.returncode==0
  server.send_signal(signal.SIGTERM);server.wait(timeout=5);assert server.returncode==0
  if enabled:
   logs=''.join(p.read_text() for p in (root/'trace').glob('*.log*'))
   assert 'CANARY_INPUT' not in logs and 'TRACE_READY' not in logs
   for stage in ['owner_dispatch','pty_accept','exchange','client_service','host_apply']: assert stage in logs,(stage,logs[:1000])
   for p in (root/'trace').glob('*.log*'):assert p.stat().st_mode&0o777==0o600 and p.stat().st_size<=4194304
  results.append({'enabled':enabled,'attach_to_first_capture_ms':attach_ms,'input_to_first_capture_ms':times,'root':str(root),'scope':'private outer PTY; capture timing includes terminal echo; no child/provider/display acceptance'})
 finally:
  for proc in [client,server]:
   if proc and proc.poll() is None:
    proc.kill()
    if master is not None:
     os.close(master);master=None
    proc.wait(timeout=3)
  if master is not None:os.close(master)
pathlib.Path('/tmp/panelink-trace-pty-results.json').write_text(json.dumps(results,indent=2));print(json.dumps(results,indent=2))
