# vanity-npub

Fast CPU vanity Nostr `npub` miner for Debian/Linux.

It generates random secp256k1 private keys, derives the Nostr public key (`x-only` secp256k1 public key), encodes it as a NIP-19 `npub`, and stops when the encoded `npub` starts with your requested prefix.

## Install Rust on Debian

Recommended:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
```

Or use Debian packages:

```bash
sudo apt update
sudo apt install -y cargo rustc
```

## Build

```bash
cargo build --release
```

The optimized binary will be at:

```bash
./target/release/vanity-npub
```

## Usage

Mine for an `npub` that starts with `npub1satoshi`:

("satoshi" is not a valid Bech32 prefix btw)

```bash
./target/release/vanity-npub satoshi
```

Equivalent full-prefix form:

```bash
./target/release/vanity-npub npub1satoshi
```

Use 8 threads:

```bash
./target/release/vanity-npub satoshi --threads 8
```

Keep finding matches:

```bash
./target/release/vanity-npub satoshi --continuous
```

Quiet output:

```bash
./target/release/vanity-npub satoshi --quiet
```

## Output

On success it prints:

- `npub`: the matching Nostr public key
- `private hex`: the matching private key as 32-byte hex
- `public hex`: the raw 32-byte Nostr public key hex

Import/use the private key carefully. Anyone with it can control that Nostr identity.

## Valid vanity characters

After `npub1`, Nostr npubs use Bech32 characters:

```text
qpzry9x8gf2tvdw0s3jn54khce6mua7l
```

Characters like `b`, `i`, and `o` are not valid Bech32 characters. The tool validates this before mining.

## Expected difficulty

Each additional vanity character after `npub1` costs about `32x` more work:

| Vanity chars | Average attempts |
| ---: | ---: |
| 1 | 32 |
| 2 | 1,024 |
| 3 | 32,768 |
| 4 | 1,048,576 |
| 5 | 33,554,432 |
| 6 | 1,073,741,824 |
| 7 | 34,359,738,368 |
| 8 | 1,099,511,627,776 |

Long prefixes can take a very long time on CPUs.

For example 424,437 attempts per second would mean an average expected time of 22.5 hours for 7 character prefix and approx. 30 days for 8 character prefix.

## Performance notes

For prefixes contained in the data section of the npub, this miner avoids allocating or fully Bech32-encoding every candidate. It compares the target Bech32 5-bit symbols directly against the x-only public key bytes, only encoding the full `npub` for a match. If your requested prefix reaches into the final checksum characters, the miner must fully encode candidates.
