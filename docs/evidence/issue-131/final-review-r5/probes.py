from pathlib import Path
import subprocess, shutil, hashlib, difflib, datetime, json, os
root=Path('/home/vaz/Code/Personal/Rust/opencode-agents-memory')
base=root/'.tmp/scratch/issue131-review-final'
out=root/'.tmp/progress/issue131-review-final'
source=(root/'crates/mem-store/src/platform.rs').read_text()
files=subprocess.check_output(['git','ls-files','-z'],cwd=root).decode().split('\0')
def replace(s,a,b):
    assert s.count(a)==1, (a,s.count(a))
    return s.replace(a,b)
m2=replace(source,'            (libc::ENOENT, None, Liveness::Alive, 1),','            (libc::ENOENT, Some(libc::ESRCH), Liveness::Dead, 1),\n            (libc::ENOENT, None, Liveness::Alive, 1),')
m2=replace(m2,'            (libc::ENOENT, Some(libc::EPERM), Liveness::Alive, 1),\n            (libc::ENOENT, Some(libc::ESRCH), Liveness::Dead, 1),','            (libc::ENOENT, Some(libc::EPERM), Liveness::Alive, 1),')
m2=replace(m2,'            assert_eq!(\n                probe.liveness(42, Some(7)),','            let actual = probe.liveness(42, Some(7));\n            println!("M2 metadata={metadata_errno} configured_signal={signal_errno:?} pid42/token7 actual={actual:?} calls={} (configured errno not observed when skipped)", probe.calls.load(std::sync::atomic::Ordering::SeqCst));\n            assert_eq!(\n                actual,')
m2mut=replace(m2,'Err(error) if error.kind() == std::io::ErrorKind::NotFound => match signal0(pid) {\n            Err(error) if error.raw_os_error() == Some(libc::ESRCH) => ObservedProcess::Absent,\n            _ => ObservedProcess::Present { start_time: None },\n        },','Err(error) if error.kind() == std::io::ErrorKind::NotFound => ObservedProcess::Present { start_time: None },')
m4=replace(source,'            let result = signal_zero_with(pid, |_, _| {\n                calls += 1;\n                Ok(())\n            });','            let result = signal_zero_with(pid, |target, signal| {\n                calls += 1;\n                println!("M4 recorder pid={pid} target={target} signal={signal} calls={calls}");\n                Ok(())\n            });\n            println!("M4 pid={pid} result={result:?} calls={calls}");')
m4mut=replace(m4,'    let target = libc::pid_t::try_from(pid)\n        .ok()\n        .filter(|target| *target > 0)\n        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;','    let target = pid as libc::pid_t;')
m5mut=replace(source,"stat.rsplit_once(')')?", "stat.split_once(')')?")
tests={'M2':'test_hidden_proc_holder_stays_alive_unless_signal_reports_esrch','M4':'test_signal_zero_never_targets_zero_negative_or_unrepresentable_pid','M5':'test_start_time_parser_handles_comm_with_spaces_and_right_parens'}
for name,control,mutant in [('M2',m2,m2mut),('M4',m4,m4mut),('M5',source,m5mut)]:
    (out/(name+'-instrumentation.diff')).write_text(''.join(difflib.unified_diff(source.splitlines(True),control.splitlines(True))))
    (out/(name+'-mutation.diff')).write_text(''.join(difflib.unified_diff(control.splitlines(True),mutant.splitlines(True))))
    for kind,text in [('production',control),('mutant',mutant)]:
        label=name+'-'+kind
        tree=base/label
        assert not tree.exists(), tree
        tree.mkdir()
        for f in files:
            if not f or f.startswith(('example/','.tmp/')): continue
            src=root/f
            if not src.is_file(): continue
            dst=tree/f; dst.parent.mkdir(parents=True,exist_ok=True); shutil.copyfile(src,dst)
        (tree/'crates/mem-store/src/platform.rs').write_text(text)
        (out/(label+'.sha256')).write_text(hashlib.sha256(text.encode()).hexdigest()+'\n')
        target=base/('target-'+label)
        cmd=['cargo','test','--quiet','-p','mem-store','--lib','platform::tests::'+tests[name],'--','--exact','--nocapture']
        meta={'label':label,'cwd':str(tree),'target':str(target),'command':cmd,'start':datetime.datetime.now(datetime.timezone.utc).isoformat()}
        print('START',label,flush=True)
        with (out/(label+'.log')).open('w') as log:
            p=subprocess.run(cmd,cwd=tree,env={**os.environ,'CARGO_TARGET_DIR':str(target)},stdout=log,stderr=subprocess.STDOUT)
        meta.update(end=datetime.datetime.now(datetime.timezone.utc).isoformat(),exit=p.returncode)
        with (out/'independent-runs.jsonl').open('a') as log: log.write(json.dumps(meta)+'\n')
        print('END',label,p.returncode,flush=True)
        if p.returncode != (0 if kind=='production' else 101): raise SystemExit('Unexpected exit '+label)
