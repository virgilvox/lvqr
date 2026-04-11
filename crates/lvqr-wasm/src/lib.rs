//! LVQR WebAssembly bindings for browser playback.
//!
//! Provides a WebTransport-based MoQ subscriber client that runs in the browser.
//! Falls back to WebSocket when WebTransport is unavailable.

mod transport;

use wasm_bindgen::prelude::*;

/// Initialize the WASM module. Called automatically on load.
#[wasm_bindgen(start)]
pub fn init() {
    console_error_panic_hook::set_once();
}

/// Returns the LVQR client library version.
#[wasm_bindgen]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Check if WebTransport is available in this browser.
#[wasm_bindgen(js_name = "isWebTransportSupported")]
pub fn is_webtransport_supported() -> bool {
    let window = match web_sys::window() {
        Some(w) => w,
        None => return false,
    };
    js_sys::Reflect::get(&window, &JsValue::from_str("WebTransport"))
        .map(|v| !v.is_undefined())
        .unwrap_or(false)
}

/// A subscriber client that connects to an LVQR relay.
///
/// Usage from JavaScript:
/// ```js
/// const client = new LvqrSubscriber("https://relay.example.com:4443");
/// await client.connect();
/// client.onFrame((data) => { /* process frame bytes */ });
/// await client.subscribe("live/my-stream", "video");
/// ```
#[wasm_bindgen]
pub struct LvqrSubscriber {
    url: String,
    transport: Option<transport::WebTransportClient>,
    on_frame_callback: Option<js_sys::Function>,
    on_error_callback: Option<js_sys::Function>,
}

#[wasm_bindgen]
impl LvqrSubscriber {
    /// Create a new subscriber targeting the given relay URL.
    #[wasm_bindgen(constructor)]
    pub fn new(url: &str) -> Self {
        Self {
            url: url.to_string(),
            transport: None,
            on_frame_callback: None,
            on_error_callback: None,
        }
    }

    /// Set the callback for received frames.
    /// The callback receives a Uint8Array of frame data.
    #[wasm_bindgen(js_name = "onFrame")]
    pub fn on_frame(&mut self, callback: js_sys::Function) {
        self.on_frame_callback = Some(callback);
    }

    /// Set the callback for errors.
    /// The callback receives an error message string.
    #[wasm_bindgen(js_name = "onError")]
    pub fn on_error(&mut self, callback: js_sys::Function) {
        self.on_error_callback = Some(callback);
    }

    /// Connect to the relay via WebTransport.
    pub async fn connect(&mut self) -> Result<(), JsValue> {
        let client = transport::WebTransportClient::connect(&self.url).await?;
        self.transport = Some(client);
        Ok(())
    }

    /// Connect to the relay with a self-signed certificate fingerprint (for development).
    /// The fingerprint should be a hex-encoded SHA-256 hash.
    #[wasm_bindgen(js_name = "connectWithFingerprint")]
    pub async fn connect_with_fingerprint(&mut self, fingerprint: &str) -> Result<(), JsValue> {
        let client = transport::WebTransportClient::connect_with_fingerprint(&self.url, fingerprint).await?;
        self.transport = Some(client);
        Ok(())
    }

    /// Open a bidirectional stream on the WebTransport connection.
    /// Returns the stream ID. Used internally for MoQ session setup.
    #[wasm_bindgen(js_name = "openBidiStream")]
    pub async fn open_bidi_stream(&self) -> Result<JsValue, JsValue> {
        let transport = self
            .transport
            .as_ref()
            .ok_or_else(|| JsValue::from_str("not connected"))?;
        let stream = transport.open_bidi().await?;
        Ok(stream)
    }

    /// Read data from a readable stream. Returns a Uint8Array.
    #[wasm_bindgen(js_name = "readStream")]
    pub async fn read_stream(&self, reader: &web_sys::ReadableStreamDefaultReader) -> Result<JsValue, JsValue> {
        transport::read_from_stream(reader).await
    }

    /// Write data to a writable stream.
    #[wasm_bindgen(js_name = "writeStream")]
    pub async fn write_stream(
        &self,
        writer: &web_sys::WritableStreamDefaultWriter,
        data: &[u8],
    ) -> Result<(), JsValue> {
        transport::write_to_stream(writer, data).await
    }

    /// Check if connected.
    #[wasm_bindgen(js_name = "isConnected")]
    pub fn is_connected(&self) -> bool {
        self.transport.is_some()
    }

    /// Close the connection.
    pub fn close(&mut self) {
        if let Some(transport) = self.transport.take() {
            transport.close();
        }
    }

    /// Get the relay URL.
    pub fn url(&self) -> String {
        self.url.clone()
    }
}

/// Log a message to the browser console.
#[wasm_bindgen]
pub fn log(msg: &str) {
    web_sys::console::log_1(&JsValue::from_str(msg));
}
