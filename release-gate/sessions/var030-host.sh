# PROOF ONLY (var-030), never merge. Runs on the pod: its host over time (load, CPU jiffies, free memory), a memory-copy
# probe each 30 s, and each minute the GPU process's CPUs, NUMA nodes, locked memory and pages per node.
gpu_bdf=$(nvidia-smi --query-gpu=pci.bus_id --format=csv,noheader | head -1 | sed 's/^0000//' | tr 'A-Z' 'a-z')
echo "# $(date -u +%T) gpu $gpu_bdf numa_node $(cat "/sys/bus/pci/devices/$gpu_bdf/numa_node" 2> /dev/null) nodes $(ls -d /sys/devices/system/node/node* 2> /dev/null | wc -l) cpus $(nproc) $(grep -m1 'model name' /proc/cpuinfo)"
i=0
while :; do
  t=$(date -u +%T)
  echo "$t load $(cut -d' ' -f1-4 /proc/loadavg) stat $(head -1 /proc/stat) avail_kb $(awk '/MemAvailable/ {print $2}' /proc/meminfo)"
  if [ $((i % 6)) = 0 ]; then
    echo "$t memcpy_gib_s $(python3 -c 'import time
b = bytearray(256 << 20); c = bytearray(256 << 20); c[:] = b; t = time.perf_counter()
for _ in range(4): c[:] = b
print(round(1 / (time.perf_counter() - t), 2))' 2>&1 | tail -1)"
  fi
  if [ $((i % 12)) = 0 ]; then
    for pid in $(nvidia-smi --query-compute-apps=pid --format=csv,noheader 2> /dev/null); do
      [ -r "/proc/$pid/status" ] || continue
      echo "$t pid $pid $(tr '\0' ' ' < "/proc/$pid/cmdline" | cut -c1-80) | $(grep -E 'Cpus_allowed_list|Mems_allowed_list|VmRSS|VmLck|VmPin' "/proc/$pid/status" | tr -s '\t\n' '  ') | numa_pages $(awk '{for (i = 1; i <= NF; i++) if ($i ~ /^N[0-9]+=/) { split($i, a, "="); n[a[1]] += a[2] }} END { for (k in n) printf "%s=%d ", k, n[k] }' "/proc/$pid/numa_maps" 2> /dev/null)"
    done
  fi
  i=$((i + 1)); sleep 5
done
