# Integrated native host proof

Native product569649a8c, evidence3df797f56: C/Rust/Python five identities agree with
one core mapping in both load orders; shared TLS and independent thread errors pass.
C11/C++17 headers and standalone C/Rust host examples pass. No GPU executes.

The native-gpu workspace wrapper compiles all three unchanged native fixtures in
the same dependency build as TensorFS and the mixed client. The dependency core's
SHA256 is unchanged before/after this helper build; receipts record all binary
NEEDED entries and hashes. They still require the exact matching core/std bundle.
The build log is outputs/xs138-native-api-integrated/helper-cohort-build-001.log.

Portable packaging and real GPU execution remain open; source/build proof grants
no allocation/reclamation, Source/GIL, full-model or release qualification.
