// Test observation only. Read current kernel accounting without walking or changing the target heap.
#include <errno.h>
#include <limits.h>
#include <libproc.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/resource.h>

int main(int argc, char **argv) {
  if (argc != 2) return 2;
  char *end = NULL;
  errno = 0;
  long value = strtol(argv[1], &end, 10);
  if (errno || !end || *end || value <= 0 || value > INT_MAX) return 2;
  struct rusage_info_v0 info = {0};
  if (proc_pid_rusage((int)value, RUSAGE_INFO_V0, (rusage_info_t *)&info) != 0) {
    perror("proc_pid_rusage");
    return 1;
  }
  if (!info.ri_proc_start_abstime || info.ri_proc_exit_abstime) return 1;
  // Keep the uint64 identity as decimal text: JavaScript Number cannot represent every start time.
  printf("{\"pid\":%ld,\"start_abstime\":\"%llu\",\"physical_bytes\":%llu,\"resident_bytes\":%llu}\n",
    value, (unsigned long long)info.ri_proc_start_abstime,
    (unsigned long long)info.ri_phys_footprint, (unsigned long long)info.ri_resident_size);
  return 0;
}
