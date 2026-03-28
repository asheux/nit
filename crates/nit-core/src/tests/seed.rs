use super::*;

#[test]
fn seed_encoders_do_not_panic_on_empty_input() {
    let input = SeedInput {
        text: "",
        source: GolSeedSource::Editor,
        file_path: None,
        version: 0,
    };
    let params = SeedParams::default();
    for encoder in [
        SeedEncoderId::AsciiBytes,
        SeedEncoderId::Lifehash16,
        SeedEncoderId::HilbertBits,
    ] {
        let encoded = encode_seed(&input, encoder, &params, 0, 0, 32, 32);
        assert_eq!(encoded.grid.width(), 32);
        assert_eq!(encoded.grid.height(), 32);
    }
}
