use std::{
    env,
    sync::OnceLock,
    time::{SystemTime, UNIX_EPOCH},
};

pub const PROPTEST_SEED_ENV: &str = "TENACIOUS_PROPTEST_SEED";

const STREAM_SALT: u64 = 0x9E37_79B9_7F4A_7C15;
const SPLITMIX_INCREMENT: u64 = 0x9E37_79B9_7F4A_7C15;
const SPLITMIX_MIX_MULTIPLIER_1: u64 = 0xBF58_476D_1CE4_E5B9;
const SPLITMIX_MIX_MULTIPLIER_2: u64 = 0x94D0_49BB_1331_11EB;
const SPLITMIX_XOR_SHIFT_1: u32 = 30;
const SPLITMIX_XOR_SHIFT_2: u32 = 27;
const SPLITMIX_XOR_SHIFT_3: u32 = 31;
const TIME_ROTATE_LEFT_BITS: u32 = 11;
const PID_ROTATE_LEFT_BITS: u32 = 23;

static RUN_SEED: OnceLock<u64> = OnceLock::new();

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(SPLITMIX_INCREMENT);
    let mut mixed = *state;
    mixed = (mixed ^ (mixed >> SPLITMIX_XOR_SHIFT_1)).wrapping_mul(SPLITMIX_MIX_MULTIPLIER_1);
    mixed = (mixed ^ (mixed >> SPLITMIX_XOR_SHIFT_2)).wrapping_mul(SPLITMIX_MIX_MULTIPLIER_2);
    mixed ^ (mixed >> SPLITMIX_XOR_SHIFT_3)
}

pub fn parse_seed(raw_seed: &str) -> Result<u64, String> {
    let normalized = raw_seed.trim().replace('_', "");
    let parsed = if let Some(hex_digits) = normalized
        .strip_prefix("0x")
        .or_else(|| normalized.strip_prefix("0X"))
    {
        u64::from_str_radix(hex_digits, 16)
    } else {
        normalized.parse::<u64>()
    };

    parsed.map_err(|err| err.to_string())
}

fn random_default_seed() -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let nanos = now.as_nanos() as u64;
    let secs = now.as_secs();
    let pid = u64::from(std::process::id());
    let mut entropy =
        nanos ^ secs.rotate_left(TIME_ROTATE_LEFT_BITS) ^ pid.rotate_left(PID_ROTATE_LEFT_BITS);
    splitmix64(&mut entropy)
}

pub fn run_seed() -> u64 {
    *RUN_SEED.get_or_init(|| match env::var(PROPTEST_SEED_ENV) {
        Ok(raw_seed) => parse_seed(&raw_seed).unwrap_or_else(|err| {
            panic!(
                "invalid {} value {:?}: {}; expected decimal or 0x-prefixed hex u64",
                PROPTEST_SEED_ENV, raw_seed, err
            )
        }),
        Err(env::VarError::NotPresent) => random_default_seed(),
        Err(env::VarError::NotUnicode(raw_seed)) => {
            panic!(
                "invalid {} value {:?}: expected valid UTF-8",
                PROPTEST_SEED_ENV, raw_seed
            )
        }
    })
}

pub fn derive_stream_seed(seed: u64, stream_discriminant: u64) -> u64 {
    let mut mixed = seed ^ stream_discriminant.wrapping_mul(STREAM_SALT);
    splitmix64(&mut mixed)
}
