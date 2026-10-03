//! SHA-256 and secure random bytes from Windows' own cryptography library (`bcrypt`), and
//! base64url, through hand-written declarations, no crates.

use std::ffi::c_void;

#[link(name = "bcrypt")]
unsafe extern "system" {
    fn BCryptHash(
        algorithm: *mut c_void,
        secret: *const u8,
        secret_len: u32,
        input: *const u8,
        input_len: u32,
        output: *mut u8,
        output_len: u32,
    ) -> i32;
    fn BCryptGenRandom(algorithm: *mut c_void, buffer: *mut u8, len: u32, flags: u32) -> i32;
}

/// `BCRYPT_SHA256_ALG_HANDLE`, a pseudo-handle that needs no opening.
const BCRYPT_SHA256_ALG_HANDLE: usize = 0x41;
/// Random bytes from the system's preferred generator, with no algorithm handle.
const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 0x2;

pub fn sha256(data: &[u8]) -> Result<[u8; 32], String> {
    let len = u32::try_from(data.len()).map_err(|_| "too large to hash")?;
    let mut hash = [0u8; 32];
    // SAFETY: the pseudo-handle needs no opening; input and output point at buffers of the
    // lengths given; there is no secret.
    let status = unsafe {
        BCryptHash(
            BCRYPT_SHA256_ALG_HANDLE as *mut c_void,
            std::ptr::null(),
            0,
            data.as_ptr(),
            len,
            hash.as_mut_ptr(),
            hash.len() as u32,
        )
    };
    if status != 0 {
        return Err(format!("SHA-256 failed (status {status:#x})"));
    }
    Ok(hash)
}

/// `N` bytes from the system's cryptographically secure random generator.
pub fn random_bytes<const N: usize>() -> Result<[u8; N], String> {
    let mut bytes = [0u8; N];
    // SAFETY: the buffer is `N` bytes long; no algorithm handle is passed, as the flag asks.
    let status = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            bytes.as_mut_ptr(),
            N as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status != 0 {
        return Err(format!("random bytes failed (status {status:#x})"));
    }
    Ok(bytes)
}

/// Base64url without padding (RFC 4648 section 5).
pub fn base64url(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
        for i in 0..=chunk.len() {
            out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_known_values() {
        let hex = |h: [u8; 32]| h.iter().map(|b| format!("{b:02x}")).collect::<String>();
        assert_eq!(
            hex(sha256(b"abc").unwrap()),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex(sha256(b"").unwrap()),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn base64url_matches_rfc_4648() {
        for (input, encoded) in [
            ("", ""),
            ("f", "Zg"),
            ("fo", "Zm8"),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg"),
            ("fooba", "Zm9vYmE"),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64url(input.as_bytes()), encoded);
        }
        // The url-safe characters, where standard base64 has `+` and `/`.
        assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
    }

    #[test]
    fn pkce_challenge_matches_rfc_7636() {
        // RFC 7636 appendix B.
        let verifier = base64url(&[
            116, 24, 223, 180, 151, 153, 224, 37, 79, 250, 96, 125, 216, 173, 187, 186, 22, 212,
            37, 77, 105, 214, 191, 240, 91, 88, 5, 88, 83, 132, 141, 121,
        ]);
        assert_eq!(verifier, "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
        assert_eq!(
            base64url(&sha256(verifier.as_bytes()).unwrap()),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn random_bytes_differ() {
        let (a, b) = (random_bytes::<32>().unwrap(), random_bytes::<32>().unwrap());
        assert_ne!(a, b);
    }
}
