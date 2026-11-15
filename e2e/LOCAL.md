# Local browser feedback

The web suite's cells run on GitHub's runners (`e2e/cli/web.sh`'s `web_suite`, one lane a job). The focused native HTTP and browser-client contract test creates and removes its own profile.

```sh
pnpm --dir web/app install --frozen-lockfile
CARGO_TARGET_DIR="$PWD/target/dev-tests" CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=1 \
COZY_PRESENCE_CLIENT_FIXTURE="$PWD/web/app/tests/project-presence.mjs" \
cargo test --locked -p calcifer --lib project::presence_tests::native_presence_orders_and_client_contract -- --exact --nocapture
```
