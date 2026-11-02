//! CPU-only release-gate probe of the real procfs namespace identity boundary.
use std::os::unix::fs::MetadataExt;

fn main() {
    let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
    assert!(!maps.contains("libcuda.so") && !maps.contains("libnvidia-ml.so"));
    println!("{}", serde_json::json!({
        "pid":std::process::id(),
        "pid_namespace_inode":std::fs::metadata("/proc/self/ns/pid").unwrap().ino(),
        "host_pid_attribution":varena::cooperative::initial_pid_namespace(),
        "driver_loaded":false,
        "nvml_loaded":false,
    }));
}
