fn main() {
    assert_eq!(varena_client::abi_version(), 1);
    let error = match varena_client::Gpu::open(-1) {
        Err(error) => error,
        Ok(_) => panic!("invalid GPU accepted"),
    };
    assert_eq!(error.status, varena_client::Status::Invalid);
    println!("external safe Rust client: ABI 1, typed Invalid, no core crate dependency");
}
