use encoding_rs::{Encoding, UTF_8, UTF_16BE, UTF_16LE};

#[derive(Debug, thiserror::Error)]
pub enum TextEncodingError {
    #[error("unsupported text encoding `{label}`")]
    UnsupportedLabel { label: String },
    #[error("{encoding} decoding failed because the input contains malformed byte sequences")]
    Decode { encoding: &'static str },
    #[error(
        "{encoding} encoding failed because the text contains characters not representable in that encoding"
    )]
    Encode { encoding: &'static str },
}

/// `TextEncodingError` 的本地化用户消息（供 `user_message` 使用）。
pub(crate) fn localized_error_message(err: &TextEncodingError) -> String {
    match err {
        TextEncodingError::UnsupportedLabel { label } => {
            crate::t!("error-invalid-text-encoding", enc = label.clone())
        }
        TextEncodingError::Decode { encoding } => {
            crate::t!("error-encoding-decode", encoding = *encoding)
        }
        TextEncodingError::Encode { encoding } => {
            crate::t!("error-encoding-encode", encoding = *encoding)
        }
    }
}

pub(crate) struct DecodedText {
    pub(crate) text: String,
    pub(crate) encoding: &'static Encoding,
    pub(crate) bom: &'static [u8],
}

#[must_use]
pub(crate) fn is_binary_output_encoding(label: &str) -> bool {
    matches!(label, "hex" | "base64")
}

pub(crate) fn encoding_from_label(label: &str) -> Result<&'static Encoding, TextEncodingError> {
    Encoding::for_label(label.trim().as_bytes()).ok_or_else(|| {
        TextEncodingError::UnsupportedLabel {
            label: label.to_string(),
        }
    })
}

#[must_use]
pub(crate) fn encoding_for_bom(data: &[u8]) -> Option<(&'static Encoding, usize)> {
    Encoding::for_bom(data)
}

fn same_encoding(a: &'static Encoding, b: &'static Encoding) -> bool {
    std::ptr::eq(a, b)
}

fn bom_for_encoding(encoding: &'static Encoding) -> &'static [u8] {
    if same_encoding(encoding, UTF_8) {
        &[0xEF, 0xBB, 0xBF]
    } else if same_encoding(encoding, UTF_16LE) {
        &[0xFF, 0xFE]
    } else if same_encoding(encoding, UTF_16BE) {
        &[0xFE, 0xFF]
    } else {
        &[]
    }
}

pub(crate) fn decode_text(
    data: &[u8],
    requested: Option<&'static Encoding>,
) -> Result<DecodedText, TextEncodingError> {
    let bom = Encoding::for_bom(data);
    let encoding = requested
        .or_else(|| bom.map(|(encoding, _)| encoding))
        .unwrap_or(UTF_8);
    let bom_len = bom
        .filter(|(bom_encoding, _)| same_encoding(*bom_encoding, encoding))
        .map_or(0, |(_, len)| len);
    let bom_bytes = if bom_len > 0 {
        bom_for_encoding(encoding)
    } else {
        &[]
    };

    decode_text_without_bom(&data[bom_len..], encoding).map(|text| DecodedText {
        text,
        encoding,
        bom: bom_bytes,
    })
}

pub(crate) fn decode_text_without_bom(
    data: &[u8],
    encoding: &'static Encoding,
) -> Result<String, TextEncodingError> {
    let (text, had_errors) = encoding.decode_without_bom_handling(data);
    if had_errors {
        return Err(TextEncodingError::Decode {
            encoding: encoding.name(),
        });
    }
    Ok(text.into_owned())
}

pub(crate) fn encode_text(
    text: &str,
    encoding: &'static Encoding,
    bom: &[u8],
) -> Result<Vec<u8>, TextEncodingError> {
    let mut out = Vec::with_capacity(bom.len() + text.len());
    out.extend_from_slice(bom);

    if same_encoding(encoding, UTF_16LE) {
        for unit in text.encode_utf16() {
            out.extend_from_slice(&unit.to_le_bytes());
        }
        return Ok(out);
    }
    if same_encoding(encoding, UTF_16BE) {
        for unit in text.encode_utf16() {
            out.extend_from_slice(&unit.to_be_bytes());
        }
        return Ok(out);
    }

    let (bytes, _, had_errors) = encoding.encode(text);
    if had_errors {
        return Err(TextEncodingError::Encode {
            encoding: encoding.name(),
        });
    }
    out.extend_from_slice(&bytes);
    Ok(out)
}
