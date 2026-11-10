use rand::{rngs::StdRng, SeedableRng};
use vsss_rs::Gf256;

fn main() -> Result<(), vsss_rs::Error> {
    // Fixed public fixture and deterministic RNG are solely for a reproducible
    // local compatibility smoke. Never use these inputs in key custody.
    let fixture = [0x42u8; 32];
    let mut rng = StdRng::from_seed([7u8; 32]);
    let shares = Gf256::split_bytes(2, 3, &fixture, &mut rng)?;
    for pair in [[0, 1], [0, 2], [1, 2]] {
        let selected = vec![shares[pair[0]].clone(), shares[pair[1]].clone()];
        assert_eq!(Gf256::combine_bytes(&selected)?, fixture);
    }
    assert!(Gf256::combine_bytes(&shares[..1]).is_err());
    assert!(Gf256::combine_bytes(&[shares[0].clone(), shares[0].clone()]).is_err());
    println!("PASS: synthetic 32-byte fixture recovered from all 3 pairs; one share and duplicate identifiers rejected.");
    println!("Boundary: primitive API compatibility only; no guardian protocol, authentication, transport, persistence, or audit qualification.");
    Ok(())
}
