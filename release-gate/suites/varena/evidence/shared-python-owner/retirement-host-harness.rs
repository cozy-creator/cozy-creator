// Output-only release-gate visibility harness: real idle native queues and fork guards.
// No Job, Arena, Region, Driver, GPU or fake CUDA owner is constructed here.
pub(crate) fn host_queue_proof() -> String {
    let mut callers=Vec::new();
    for i in 0..16 {
        callers.push(std::thread::spawn(move || endpoint(185+(i%2)).unwrap()));
    }
    let endpoints=callers.into_iter().map(|thread|thread.join().unwrap()).collect::<Vec<_>>();
    for (i,endpoint) in endpoints.iter().enumerate() {
        assert!(Arc::ptr_eq(endpoint,&endpoints[i%2]));
    }
    assert!(!Arc::ptr_eq(&endpoints[0],&endpoints[1]));
    assert_eq!(QUEUES.get().unwrap().lock().unwrap().len(),2);
    // Preparing an idle host queue must not acquire the CUDA exclusion mutex.
    let held=crate::capture_counts().unwrap();
    let (send,receive)=mpsc::sync_channel(1);
    let startup=std::thread::spawn(move || send.send(prepare(187).is_ok()).unwrap());
    assert!(receive.recv_timeout(Duration::from_secs(2)).unwrap());
    startup.join().unwrap();
    drop(held);
    let end=std::time::Instant::now()+Duration::from_secs(2);
    let native_workers=loop {
        let workers=std::fs::read_dir("/proc/self/task").unwrap().filter_map(|entry| {
            std::fs::read_to_string(entry.ok()?.path().join("comm")).ok()
        }).filter(|name|name.starts_with("varena-retire-")).count();
        if workers==3 {break workers;}
        assert!(std::time::Instant::now()<end,"native worker startup did not become observable");
        std::thread::yield_now();
    };
    inherited_queue_proof();
    let maps=std::fs::read_to_string("/proc/self/maps").unwrap();
    assert!(!maps.contains("libcuda.so")&&!maps.contains("libnvidia-ml.so"));
    format!("{{\"passed\":true,\"concurrent_preparations\":16,\"per_device_endpoints\":3,\"actual_idle_workers\":{},\"startup_avoided_capture_mutex\":true,\"fork_rejected_before_inherited_queue_mutex\":true,\"gpu_owners_constructed\":0,\"cuda_loaded\":false,\"nvml_loaded\":false,\"gpu_retirement_qualified\":false}}",native_workers)
}

#[allow(unsafe_code)]
fn inherited_queue_proof() {
    let held=QUEUES.get().unwrap().lock().unwrap();
    let child=unsafe{libc::fork()};
    assert!(child>=0);
    if child==0 {
        let refused=prepare(185).is_err();
        unsafe{libc::_exit(if refused{0}else{1});}
    }
    let deadline=std::time::Instant::now()+Duration::from_secs(2);
    let mut status=0;
    loop {
        let result=unsafe{libc::waitpid(child,&mut status,libc::WNOHANG)};
        if result==child{break;}
        assert!(result>=0);
        if std::time::Instant::now()>=deadline {
            unsafe{libc::kill(child,libc::SIGKILL);libc::waitpid(child,&mut status,0);}
            panic!("inherited queue mutex was touched");
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    drop(held);
    assert!(libc::WIFEXITED(status)&&libc::WEXITSTATUS(status)==0);
}
