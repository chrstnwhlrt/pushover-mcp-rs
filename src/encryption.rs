//! End-to-end encryption of the message fields, as Pushover's API
//! documentation describes it (<https://pushover.net/api#e2ee>): each field
//! is compressed with gzip, encrypted with AES-256-CBC (PKCS#7 padding)
//! under a random 16-byte IV and authenticated with HMAC-SHA256 over IV and
//! ciphertext, with the same key; the field value is the Base64 of IV,
//! ciphertext and tag. Pushover's iOS and Android apps decrypt it from
//! version 5.0 with the key the user stores there; Pushover never sees it.

use std::io::Write;

use aws_lc_rs::{
    cipher::{AES_256, EncryptionContext, PaddedBlockEncryptingKey, UnboundCipherKey},
    hmac,
    iv::FixedLength,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use flate2::{Compression, write::GzEncoder};

/// The fields Pushover's apps decrypt; all others go in the clear.
const ENCRYPTED_FIELDS: [&str; 4] = ["message", "title", "url", "url_title"];

const IV_BYTES: usize = 16;

/// A 256-bit key. Its bytes never reach a log or an error text.
pub struct Key([u8; 32]);

impl Key {
    /// The key from its 64 hexadecimal characters, as the apps take it.
    pub fn from_hex(text: &str) -> Option<Self> {
        let (pairs, rest) = text.as_bytes().as_chunks::<2>();
        if pairs.len() != 32 || !rest.is_empty() {
            return None;
        }
        let mut key = [0; 32];
        for (byte, [high, low]) in key.iter_mut().zip(pairs) {
            *byte = (hex_digit(*high)? << 4) | hex_digit(*low)?;
        }
        Some(Self(key))
    }

    /// Encrypts the fields the apps decrypt and marks the request with
    /// `encrypted=1`.
    pub fn encrypt_fields(&self, fields: &mut Vec<(&'static str, String)>) -> Result<(), String> {
        for (name, value) in fields.iter_mut() {
            if ENCRYPTED_FIELDS.contains(name) {
                *value = self.encrypt(value)?;
            }
        }
        fields.push(("encrypted", "1".to_owned()));
        Ok(())
    }

    fn encrypt(&self, plaintext: &str) -> Result<String, String> {
        let mut compressed = GzEncoder::new(Vec::new(), Compression::best());
        compressed
            .write_all(plaintext.as_bytes())
            .map_err(|e| format!("Could not compress for encryption: {e}"))?;
        let compressed = compressed
            .finish()
            .map_err(|e| format!("Could not compress for encryption: {e}"))?;
        let mut iv = [0; IV_BYTES];
        aws_lc_rs::rand::fill(&mut iv).map_err(|_| "Could not create a random IV".to_owned())?;
        self.seal(iv, compressed)
    }

    /// IV, ciphertext and tag of `data`, in Base64.
    fn seal(&self, iv: [u8; IV_BYTES], mut data: Vec<u8>) -> Result<String, String> {
        let failed = |_| "Encryption failed".to_owned();
        let key = UnboundCipherKey::new(&AES_256, &self.0).map_err(failed)?;
        PaddedBlockEncryptingKey::cbc_pkcs7(key)
            .map_err(failed)?
            .less_safe_encrypt(&mut data, EncryptionContext::Iv128(FixedLength::from(iv)))
            .map_err(failed)?;

        let mut sealed = Vec::with_capacity(IV_BYTES + data.len() + 32);
        sealed.extend_from_slice(&iv);
        sealed.extend_from_slice(&data);
        let tag = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, &self.0), &sealed);
        sealed.extend_from_slice(tag.as_ref());
        Ok(STANDARD.encode(sealed))
    }
}

/// The value of one hexadecimal digit; no sign, unlike `u8::from_str_radix`.
fn hex_digit(byte: u8) -> Option<u8> {
    char::from(byte)
        .to_digit(16)
        .and_then(|digit| u8::try_from(digit).ok())
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use aws_lc_rs::cipher::{DecryptionContext, PaddedBlockDecryptingKey};

    use super::*;

    const HEX: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

    /// What an app does with a field: check the tag, decrypt, unpack.
    fn open(key_hex: &str, value: &str) -> String {
        let key = Key::from_hex(key_hex).unwrap();
        let sealed = STANDARD.decode(value).unwrap();
        let (signed, tag) = sealed.split_at(sealed.len() - 32);
        hmac::verify(&hmac::Key::new(hmac::HMAC_SHA256, &key.0), signed, tag).unwrap();
        let (iv, ciphertext) = signed.split_at(IV_BYTES);
        let mut ciphertext = ciphertext.to_vec();
        let cipher =
            PaddedBlockDecryptingKey::cbc_pkcs7(UnboundCipherKey::new(&AES_256, &key.0).unwrap())
                .unwrap();
        let iv: [u8; IV_BYTES] = iv.try_into().unwrap();
        let compressed = cipher
            .decrypt(
                &mut ciphertext,
                DecryptionContext::Iv128(FixedLength::from(iv)),
            )
            .unwrap();
        let mut plaintext = String::new();
        flate2::read::GzDecoder::new(&compressed[..])
            .read_to_string(&mut plaintext)
            .unwrap();
        plaintext
    }

    #[test]
    fn keys_are_64_hexadecimal_characters() {
        assert!(Key::from_hex(HEX).is_some());
        assert!(Key::from_hex(&HEX.to_uppercase()).is_some());
        assert!(Key::from_hex(&HEX[..62]).is_none());
        assert!(Key::from_hex(&format!("{HEX}00")).is_none());
        assert!(Key::from_hex(&HEX.replace('a', "g")).is_none());
        assert!(Key::from_hex(&HEX.replacen("00", "+0", 1)).is_none());
        // 64 bytes, but not 64 characters of one byte each
        assert!(Key::from_hex(&format!("{}ää", &HEX[..60])).is_none());
    }

    /// Computed with the tools of Pushover's example: `openssl enc
    /// -aes-256-cbc -K <key> -iv <iv>`, `openssl dgst -sha256 -mac HMAC
    /// -macopt hexkey:<key>` over IV and ciphertext, `openssl base64 -A`.
    #[test]
    fn the_layout_is_iv_ciphertext_and_tag() {
        let key = Key::from_hex(HEX).unwrap();
        let iv = *b"0123456789abcdef";
        assert_eq!(
            key.seal(iv, b"seventeen bytes!!".to_vec()).unwrap(),
            "MDEyMzQ1Njc4OWFiY2RlZptt5sf18GkRDPeDUiFPt6UEedPC3KINjnmli0hcIyx4QeanEHacn5JZ2tqPyvyRg\
             ACNkYMRFH3EywlO7CwGWUQ="
        );
    }

    #[test]
    fn fields_decrypt_to_what_was_sent() {
        let key = Key::from_hex(HEX).unwrap();
        let mut fields = vec![
            ("message", "Grüße aus der Küche \u{1F680}".to_owned()),
            ("title", "Otomo: Essen".to_owned()),
            ("url", "https://example.com/rezept?id=1&x=2".to_owned()),
            ("url_title", "Rezept".to_owned()),
            ("priority", "1".to_owned()),
            ("sound", "kuechengong".to_owned()),
        ];
        let sent = fields.clone();
        key.encrypt_fields(&mut fields).unwrap();

        assert_eq!(fields.len(), sent.len() + 1);
        assert_eq!(fields.last().unwrap(), &("encrypted", "1".to_owned()));
        for ((name, value), (_, plain)) in fields.iter().zip(&sent) {
            if ENCRYPTED_FIELDS.contains(name) {
                assert_ne!(value, plain, "{name}");
                assert_eq!(open(HEX, value), *plain, "{name}");
            } else {
                assert_eq!(value, plain, "{name}");
            }
        }
        // A fresh IV for every field and every message
        let again = key.encrypt("Otomo: Essen").unwrap();
        assert_ne!(again, fields[1].1);
        assert_eq!(open(HEX, &again), "Otomo: Essen");
    }
}
