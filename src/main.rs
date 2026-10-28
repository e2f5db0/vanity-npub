use bech32::{ToBase32, Variant};
use rand::rngs::OsRng;
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use std::env;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

const HRP: &str = "npub";
const PREFIX: &str = "npub1";
const BECH32_CHARSET: &[u8; 32] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";
const NPUB_DATA_CHARS: usize = 52;
const NPUB_TOTAL_CHARS: usize = 63;

#[derive(Debug)]
struct Args {
    desired: String,
    threads: Option<usize>,
    progress_interval: u64,
    continuous: bool,
    quiet: bool,
}

#[derive(Clone, Debug)]
struct Target {
    full_prefix: String,
    data_values: Vec<u8>,
    requires_full_encode: bool,
}

#[derive(Debug)]
struct Match {
    npub: String,
    private_key_hex: String,
    pubkey_hex: String,
    attempts: u64,
    thread_id: usize,
}

fn parse_args<I>(args: I) -> Result<Args, String>
where
    I: IntoIterator<Item = String>,
{
    let mut desired = None;
    let mut threads = None;
    let mut progress_interval = 5_u64;
    let mut continuous = false;
    let mut quiet = false;

    let mut args = args.into_iter();
    let _program = args.next();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            "-c" | "--continuous" => continuous = true,
            "-q" | "--quiet" => quiet = true,
            "-t" | "--threads" => {
                let value = args
                    .next()
                    .ok_or_else(|| format!("{arg} requires a value"))?;
                let parsed = value
                    .parse::<usize>()
                    .map_err(|_| format!("invalid thread count: {value}"))?;
                if parsed == 0 {
                    return Err("thread count must be at least 1".to_string());
                }
                threads = Some(parsed);
            }
            "-i" | "--progress-interval" => {
                let value = args
                    .next()
                    .ok_or_else(|| format!("{arg} requires a value"))?;
                progress_interval = value
                    .parse::<u64>()
                    .map_err(|_| format!("invalid progress interval: {value}"))?;
            }
            _ if arg.starts_with("--threads=") => {
                let value = &arg[10..];
                let parsed = value
                    .parse::<usize>()
                    .map_err(|_| format!("invalid thread count: {value}"))?;
                if parsed == 0 {
                    return Err("thread count must be at least 1".to_string());
                }
                threads = Some(parsed);
            }
            _ if arg.starts_with("--progress-interval=") => {
                let value = &arg[20..];
                progress_interval = value
                    .parse::<u64>()
                    .map_err(|_| format!("invalid progress interval: {value}"))?;
            }
            _ if arg.starts_with('-') => return Err(format!("unknown option: {arg}")),
            _ => {
                if desired.replace(arg).is_some() {
                    return Err("only one desired prefix may be provided".to_string());
                }
            }
        }
    }

    Ok(Args {
        desired: desired.ok_or_else(|| "missing desired prefix".to_string())?,
        threads,
        progress_interval,
        continuous,
        quiet,
    })
}

fn print_usage() {
    eprintln!(
        "vanity-npub 0.1.0\n\nUSAGE:\n    vanity-npub [OPTIONS] <desired>\n\nARGS:\n    <desired>    Prefix after npub1, or a full prefix beginning with npub1\n\nOPTIONS:\n    -t, --threads <N>              Number of mining threads [default: logical CPUs]\n    -i, --progress-interval <SEC>  Print progress every N seconds; 0 disables [default: 5]\n    -c, --continuous               Continue mining after finding a match\n    -q, --quiet                    Suppress status/progress lines\n    -h, --help                     Print help\n\nEXAMPLES:\n    vanity-npub satoshi\n    vanity-npub npub1satoshi --threads 8"
    );
}

fn main() {
    let args = match parse_args(env::args()) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("error: {message}\n");
            print_usage();
            std::process::exit(2);
        }
    };

    let target = match parse_target(&args.desired) {
        Ok(target) => target,
        Err(err) => {
            eprintln!("error: {err}");
            std::process::exit(2);
        }
    };

    let threads = args.threads.unwrap_or_else(num_cpus::get).max(1);

    if !args.quiet {
        eprintln!("target prefix: {}", target.full_prefix);
        eprintln!("threads:       {threads}");
        if target.requires_full_encode {
            eprintln!("mode:          full npub encode per candidate (prefix reaches checksum)");
        } else {
            eprintln!("mode:          fast 5-bit data prefix check");
        }
        eprintln!("press Ctrl-C to stop\n");
    }

    let found = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    let attempts = Arc::new(AtomicU64::new(0));
    let (tx, rx) = mpsc::channel::<Match>();

    {
        let stop = Arc::clone(&stop);
        ctrlc::set_handler(move || {
            stop.store(true, Ordering::Relaxed);
        })
        .expect("failed to install Ctrl-C handler");
    }

    for thread_id in 0..threads {
        let tx = tx.clone();
        let target = target.clone();
        let found = Arc::clone(&found);
        let stop = Arc::clone(&stop);
        let attempts = Arc::clone(&attempts);
        let continuous = args.continuous;

        thread::spawn(move || {
            mine(thread_id, target, continuous, found, stop, attempts, tx);
        });
    }
    drop(tx);

    let started = Instant::now();
    let progress_interval = Duration::from_secs(args.progress_interval);
    let mut last_progress = Instant::now();
    let mut last_attempts = 0_u64;

    loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(result) => {
                println!("npub:        {}", result.npub);
                println!("private hex: {}", result.private_key_hex);
                println!("public hex:  {}", result.pubkey_hex);
                println!("attempts:    {}", result.attempts);
                println!("thread:      {}", result.thread_id);
                println!();

                if !args.continuous {
                    stop.store(true, Ordering::Relaxed);
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        if stop.load(Ordering::Relaxed) {
            break;
        }

        if !args.quiet && args.progress_interval > 0 && last_progress.elapsed() >= progress_interval {
            let total = attempts.load(Ordering::Relaxed);
            let elapsed = started.elapsed().as_secs_f64();
            let interval_elapsed = last_progress.elapsed().as_secs_f64();
            let interval_rate = (total - last_attempts) as f64 / interval_elapsed;
            let avg_rate = total as f64 / elapsed.max(0.001);
            eprintln!(
                "{total} attempts | {:.0}/s current | {:.0}/s avg | elapsed {}",
                interval_rate,
                avg_rate,
                format_duration(started.elapsed())
            );
            last_progress = Instant::now();
            last_attempts = total;
        }
    }

    stop.store(true, Ordering::Relaxed);

    if !args.quiet {
        let total = attempts.load(Ordering::Relaxed);
        let elapsed = started.elapsed().as_secs_f64();
        eprintln!(
            "done: {total} attempts in {} ({:.0}/s avg)",
            format_duration(started.elapsed()),
            total as f64 / elapsed.max(0.001)
        );
    }

}

fn mine(
    thread_id: usize,
    target: Target,
    continuous: bool,
    found: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    attempts: Arc<AtomicU64>,
    tx: mpsc::Sender<Match>,
) {
    let secp = Secp256k1::new();
    let mut rng = OsRng;
    let mut local_attempts = 0_u64;

    while !stop.load(Ordering::Relaxed) {
        if !continuous && found.load(Ordering::Relaxed) {
            break;
        }

        let secret_key = SecretKey::new(&mut rng);
        let public_key = PublicKey::from_secret_key(&secp, &secret_key);
        let (xonly, _parity) = public_key.x_only_public_key();
        let pubkey_bytes = xonly.serialize();

        local_attempts += 1;
        if local_attempts >= 1024 {
            attempts.fetch_add(local_attempts, Ordering::Relaxed);
            local_attempts = 0;
        }

        if matches_target(&target, &pubkey_bytes) {
            let total_attempts = attempts.fetch_add(local_attempts, Ordering::Relaxed) + local_attempts;
            local_attempts = 0;

            if !continuous && found.swap(true, Ordering::Relaxed) {
                let mut secret_bytes = secret_key.secret_bytes();
                secret_bytes.fill(0);
                break;
            }

            let npub = encode_npub(&pubkey_bytes);
            let mut secret_bytes = secret_key.secret_bytes();
            let private_key_hex = hex::encode(secret_bytes);
            secret_bytes.fill(0);
            let pubkey_hex = hex::encode(pubkey_bytes);

            let _ = tx.send(Match {
                npub,
                private_key_hex,
                pubkey_hex,
                attempts: total_attempts,
                thread_id,
            });

            if !continuous {
                break;
            }
        }
    }

    if local_attempts > 0 {
        attempts.fetch_add(local_attempts, Ordering::Relaxed);
    }
}

fn parse_target(input: &str) -> Result<Target, String> {
    let wanted = input.trim().to_ascii_lowercase();
    if wanted.is_empty() {
        return Err("desired prefix cannot be empty".to_string());
    }

    let full_prefix = if wanted.starts_with(PREFIX) {
        wanted
    } else if wanted.starts_with(HRP) {
        return Err("prefix starting with 'npub' must start with 'npub1'".to_string());
    } else {
        format!("{PREFIX}{wanted}")
    };

    if full_prefix.len() > NPUB_TOTAL_CHARS {
        return Err(format!(
            "prefix is too long: npub strings are {NPUB_TOTAL_CHARS} characters"
        ));
    }

    if !full_prefix.starts_with(PREFIX) {
        return Err(format!("prefix must start with {PREFIX}"));
    }

    let wanted_data = &full_prefix[PREFIX.len()..];
    let mut data_values = Vec::with_capacity(wanted_data.len().min(NPUB_DATA_CHARS));

    for ch in wanted_data.bytes().take(NPUB_DATA_CHARS) {
        match BECH32_CHARSET.iter().position(|&candidate| candidate == ch) {
            Some(value) => data_values.push(value as u8),
            None => {
                return Err(format!(
                    "invalid bech32 character '{}'; allowed characters are {}",
                    ch as char,
                    std::str::from_utf8(BECH32_CHARSET).expect("charset is utf8")
                ));
            }
        }
    }

    let wanted_data_len = wanted_data.len();

    Ok(Target {
        full_prefix,
        data_values,
        requires_full_encode: wanted_data_len > NPUB_DATA_CHARS,
    })
}

fn matches_target(target: &Target, pubkey_bytes: &[u8; 32]) -> bool {
    if !target.data_values.is_empty() && !pubkey_matches_data_prefix(pubkey_bytes, &target.data_values) {
        return false;
    }

    if target.requires_full_encode {
        encode_npub(pubkey_bytes).starts_with(&target.full_prefix)
    } else {
        true
    }
}

fn pubkey_matches_data_prefix(pubkey_bytes: &[u8; 32], wanted: &[u8]) -> bool {
    let mut accumulator: u16 = 0;
    let mut bits: u8 = 0;
    let mut produced = 0_usize;

    for &byte in pubkey_bytes {
        accumulator = (accumulator << 8) | byte as u16;
        bits += 8;

        while bits >= 5 {
            bits -= 5;
            let value = ((accumulator >> bits) & 0x1f) as u8;
            if produced < wanted.len() {
                if value != wanted[produced] {
                    return false;
                }
                produced += 1;
                if produced == wanted.len() {
                    return true;
                }
            }
        }
    }

    if bits > 0 && produced < wanted.len() {
        let value = ((accumulator << (5 - bits)) & 0x1f) as u8;
        if value != wanted[produced] {
            return false;
        }
        produced += 1;
    }

    produced == wanted.len()
}

fn encode_npub(pubkey_bytes: &[u8; 32]) -> String {
    bech32::encode(HRP, pubkey_bytes.to_base32(), Variant::Bech32).expect("valid npub encoding")
}

fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    let seconds = secs % 60;

    let mut out = String::new();
    if hours > 0 {
        let _ = write!(out, "{hours}h{minutes:02}m{seconds:02}s");
    } else if minutes > 0 {
        let _ = write!(out, "{minutes}m{seconds:02}s");
    } else {
        let _ = write!(out, "{seconds}s");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_prefix_match_agrees_with_bech32_encoding() {
        let key = [42_u8; 32];
        let npub = encode_npub(&key);
        let suffix = &npub[PREFIX.len()..PREFIX.len() + 12];
        let target = parse_target(suffix).unwrap();
        assert!(matches_target(&target, &key));
    }

    #[test]
    fn accepts_full_npub_prefix() {
        let key = [7_u8; 32];
        let npub = encode_npub(&key);
        let target = parse_target(&npub[..14]).unwrap();
        assert!(matches_target(&target, &key));
    }

    #[test]
    fn rejects_invalid_bech32_character() {
        assert!(parse_target("bio").is_err());
    }
}
