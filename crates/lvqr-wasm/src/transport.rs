//! WebTransport client for browser connections.
//!
//! Wraps the browser's native WebTransport API via web-sys,
//! providing a Rust-friendly interface for WASM code.

use js_sys::{Object, Reflect, Uint8Array};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    ReadableStreamDefaultReader, WebTransport, WebTransportBidirectionalStream, WebTransportHash, WebTransportOptions,
    WritableStreamDefaultWriter,
};

/// A WebTransport connection to an LVQR relay.
pub struct WebTransportClient {
    inner: WebTransport,
}

impl WebTransportClient {
    /// Connect to a relay using system TLS roots (production).
    pub async fn connect(url: &str) -> Result<Self, JsValue> {
        let options = WebTransportOptions::new();
        let inner = WebTransport::new_with_options(url, &options)?;
        JsFuture::from(inner.ready()).await?;
        Ok(Self { inner })
    }

    /// Connect with a self-signed certificate fingerprint (development).
    pub async fn connect_with_fingerprint(url: &str, fingerprint_hex: &str) -> Result<Self, JsValue> {
        let hash_bytes =
            hex_to_bytes(fingerprint_hex).map_err(|e| JsValue::from_str(&format!("invalid fingerprint hex: {e}")))?;

        let hash = WebTransportHash::new();
        hash.set_algorithm("sha-256");
        let arr = Uint8Array::new_with_length(hash_bytes.len() as u32);
        arr.copy_from(&hash_bytes);
        hash.set_value(&arr);

        let hashes = js_sys::Array::new();
        hashes.push(&hash);

        let options = WebTransportOptions::new();
        Reflect::set(&options, &JsValue::from_str("serverCertificateHashes"), &hashes)?;

        let inner = WebTransport::new_with_options(url, &options)?;
        JsFuture::from(inner.ready()).await?;
        Ok(Self { inner })
    }

    /// Open a bidirectional stream.
    /// Returns a JS object with `readable` and `writable` properties.
    pub async fn open_bidi(&self) -> Result<JsValue, JsValue> {
        let stream: WebTransportBidirectionalStream = JsFuture::from(self.inner.create_bidirectional_stream())
            .await?
            .unchecked_into();

        let result = Object::new();
        Reflect::set(&result, &JsValue::from_str("readable"), &stream.readable())?;
        Reflect::set(&result, &JsValue::from_str("writable"), &stream.writable())?;
        Ok(result.into())
    }

    /// Close the connection.
    pub fn close(&self) {
        self.inner.close();
    }
}

/// Read a chunk from a ReadableStream reader.
pub async fn read_from_stream(reader: &ReadableStreamDefaultReader) -> Result<JsValue, JsValue> {
    let result = JsFuture::from(reader.read()).await?;
    let done = Reflect::get(&result, &JsValue::from_str("done"))?
        .as_bool()
        .unwrap_or(true);

    if done {
        return Err(JsValue::from_str("stream ended"));
    }

    Reflect::get(&result, &JsValue::from_str("value"))
}

/// Write data to a WritableStream writer.
pub async fn write_to_stream(writer: &WritableStreamDefaultWriter, data: &[u8]) -> Result<(), JsValue> {
    let arr = Uint8Array::new_with_length(data.len() as u32);
    arr.copy_from(data);
    JsFuture::from(writer.write_with_chunk(&arr)).await?;
    Ok(())
}

/// Convert a hex string to bytes.
fn hex_to_bytes(hex: &str) -> Result<Vec<u8>, String> {
    let hex = hex.replace([':', '-', ' '], "");
    if hex.len() % 2 != 0 {
        return Err("odd-length hex string".into());
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hex_to_bytes() {
        assert_eq!(hex_to_bytes("deadbeef").unwrap(), vec![0xde, 0xad, 0xbe, 0xef]);
        assert_eq!(hex_to_bytes("DE:AD:BE:EF").unwrap(), vec![0xde, 0xad, 0xbe, 0xef]);
        assert_eq!(hex_to_bytes("de-ad-be-ef").unwrap(), vec![0xde, 0xad, 0xbe, 0xef]);
        assert!(hex_to_bytes("xyz").is_err());
    }
}
