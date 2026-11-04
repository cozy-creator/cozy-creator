# Source provider fixture

The real C public-Source provider the fault cells drive (`core/faults`). Build it against the wheel's `.native` directory:

```sh
nice -n 19 cc -std=c11 -Wall -Wextra -Werror -fPIC -shared \
  -I crates/varena/include \
  release-gate/suites/varena/native-tensor-export/source_provider.c \
  -L NATIVE_LIB_DIR -Wl,-rpath,NATIVE_LIB_DIR -lvarena -ldl -pthread -o provider.so
```
