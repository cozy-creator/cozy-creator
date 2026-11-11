New projects are encrypted by default. `cozy new` collects a passphrase locally and prints or saves
an off-device recovery secret; automation supplies `--passphrase-file FILE|-`, or explicitly opts
out with `--unencrypted`. WebUI creation, including an empty canvas's first import, uses the same
setup and waits for recovery acknowledgment before uploading files. The first database is written
inside encrypted storage; a new encrypted directory must be empty. Existing projects keep their
storage. Metadata-only `cozy run --describe` needs no new project.
