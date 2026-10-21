//! The built-in model catalog (task `p9-transcribe-downloader`, decision D1).
//!
//! Every entry pins the exact Hugging Face repository revision, file name,
//! byte size and SHA-256 of the GGUF file, so a download can be verified
//! completely. Licenses were read from the model cards of the upstream
//! `openai/whisper-*` repositories on Hugging Face (checked 2026-10-15):
//! `openai/whisper-tiny` is Apache-2.0 and `openai/whisper-large-v3-turbo`
//! is MIT; both GGUF repositories from `handy-computer` carry the matching
//! upstream license tag.

/// One downloadable model in the built-in catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogEntry {
    /// Stable catalog id: the model id of a file in the models folder with
    /// this entry's size and SHA-256, whatever its file name.
    pub id: &'static str,
    /// One-line human description for listings.
    pub description: &'static str,
    /// Download URL pinned to a full repository revision (HTTPS only).
    pub url: &'static str,
    /// File name `models download` stores the verified bytes under in the
    /// models folder.
    pub file_name: &'static str,
    /// Exact expected file size in bytes.
    pub size_bytes: u64,
    /// Lowercase hex SHA-256 of the file.
    pub sha256: &'static str,
    /// License of the model weights, from the upstream model card.
    pub license: &'static str,
}

/// The whole catalog. Keep it small: only models whose bytes and hashes were
/// verified in this project belong here (D1).
pub const CATALOG: &[CatalogEntry] = &[
    CatalogEntry {
        id: "whisper-tiny-q8_0",
        description: "OpenAI Whisper tiny, Q8_0 GGUF quantization; fast and rough, useful for testing",
        url: "https://huggingface.co/handy-computer/whisper-tiny-gguf/resolve/6687f30c99641ee265df421e582354adbc8848fc/whisper-tiny-Q8_0.gguf",
        file_name: "whisper-tiny-Q8_0.gguf",
        size_bytes: 45_981_088,
        sha256: "325b9c7997cd1eff81ef709d55766565e71be696130cc3a3d444713798706834",
        license: "Apache-2.0 (OpenAI Whisper weights)",
    },
    CatalogEntry {
        id: "whisper-large-v3-turbo-q8_0",
        description: "OpenAI Whisper large-v3-turbo, Q8_0 GGUF quantization; slower and usually more accurate",
        url: "https://huggingface.co/handy-computer/whisper-large-v3-turbo-gguf/resolve/5eaf945c7978e564bae5b28a5b1639dd93c2bfb1/whisper-large-v3-turbo-Q8_0.gguf",
        file_name: "whisper-large-v3-turbo-Q8_0.gguf",
        size_bytes: 886_381_760,
        sha256: "b2e30cc286bc9f3aba4db9099fc7403543497c05ce7100d0d83091ddfd25a183",
        license: "MIT (OpenAI Whisper weights)",
    },
];

/// Looks up a catalog entry by id.
pub fn find(id: &str) -> Option<&'static CatalogEntry> {
    CATALOG.iter().find(|entry| entry.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_ids_resolve() {
        let tiny = find("whisper-tiny-q8_0").unwrap();
        assert_eq!(tiny.size_bytes, 45_981_088);
        assert_eq!(tiny.file_name, "whisper-tiny-Q8_0.gguf");
        assert!(tiny.url.starts_with("https://huggingface.co/"));
        assert!(
            tiny.url
                .contains("/resolve/6687f30c99641ee265df421e582354adbc8848fc/")
        );
        let turbo = find("whisper-large-v3-turbo-q8_0").unwrap();
        assert_eq!(turbo.size_bytes, 886_381_760);
        assert!(
            turbo
                .url
                .contains("/resolve/5eaf945c7978e564bae5b28a5b1639dd93c2bfb1/")
        );
    }

    #[test]
    fn unknown_id_is_none() {
        assert!(find("whisper-base").is_none());
    }

    /// Catalog ids stay plain: `[a-z0-9][a-z0-9._-]*`.
    fn is_plain_id(id: &str) -> bool {
        let mut bytes = id.bytes();
        match bytes.next() {
            Some(b) if b.is_ascii_lowercase() || b.is_ascii_digit() => {}
            _ => return false,
        }
        bytes.all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'_' || b == b'-'
        })
    }

    #[test]
    fn ids_are_unique_and_plain() {
        for entry in CATALOG {
            assert!(is_plain_id(entry.id));
        }
        let mut ids: Vec<&str> = CATALOG.iter().map(|entry| entry.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), CATALOG.len());
    }

    #[test]
    fn sha256_are_64_lowercase_hex() {
        for entry in CATALOG {
            assert_eq!(entry.sha256.len(), 64);
            assert!(
                entry
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
                "sha256 must be lowercase hex"
            );
        }
    }
}
