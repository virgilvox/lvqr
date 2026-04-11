//! LVQR WebAssembly bindings for browser playback.
//!
//! Provides a WebTransport-based client that runs in the browser.
//! Falls back to WebSocket when WebTransport is unavailable.
//!
//! This crate must be compiled with `--target wasm32-unknown-unknown`.
//! Building for native targets provides only version/utility exports.

#[cfg(target_arch = "wasm32")]
mod transport;

use wasm_bindgen::prelude::*;

/// Initialize the WASM module. Called automatically on load.
#[wasm_bindgen(start)]
pub fn init() {
    #[cfg(target_arch = "wasm32")]
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
    #[cfg(target_arch = "wasm32")]
    {
        let window = match web_sys::window() {
            Some(w) => w,
            None => return false,
        };
        js_sys::Reflect::get(&window, &JsValue::from_str("WebTransport"))
            .map(|v| !v.is_undefined())
            .unwrap_or(false)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        false
    }
}

/// A subscriber client that connects to an LVQR relay.
///
/// Usage from JavaScript:
/// ```js
/// const client = new LvqrSubscriber("https://relay.example.com:4443");
/// await client.connect();
/// client.onFrame((data) => { /* process frame bytes */ });
/// ```
#[wasm_bindgen]
pub struct LvqrSubscriber {
    url: String,
    #[cfg(target_arch = "wasm32")]
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
            #[cfg(target_arch = "wasm32")]
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
        #[cfg(target_arch = "wasm32")]
        {
            let client = transport::WebTransportClient::connect(&self.url).await?;
            self.transport = Some(client);
            Ok(())
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            Err(JsValue::from_str("WebTransport requires wasm32 target"))
        }
    }

    /// Connect to the relay with a self-signed certificate fingerprint (for development).
    /// The fingerprint should be a hex-encoded SHA-256 hash.
    #[wasm_bindgen(js_name = "connectWithFingerprint")]
    pub async fn connect_with_fingerprint(&mut self, fingerprint: &str) -> Result<(), JsValue> {
        #[cfg(target_arch = "wasm32")]
        {
            let client = transport::WebTransportClient::connect_with_fingerprint(&self.url, fingerprint).await?;
            self.transport = Some(client);
            Ok(())
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = fingerprint;
            Err(JsValue::from_str("WebTransport requires wasm32 target"))
        }
    }

    /// Open a bidirectional stream on the WebTransport connection.
    #[wasm_bindgen(js_name = "openBidiStream")]
    pub async fn open_bidi_stream(&self) -> Result<JsValue, JsValue> {
        #[cfg(target_arch = "wasm32")]
        {
            let transport = self
                .transport
                .as_ref()
                .ok_or_else(|| JsValue::from_str("not connected"))?;
            transport.open_bidi().await
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            Err(JsValue::from_str("WebTransport requires wasm32 target"))
        }
    }

    /// Read data from a readable stream. Returns a Uint8Array.
    #[wasm_bindgen(js_name = "readStream")]
    pub async fn read_stream(&self, reader: JsValue) -> Result<JsValue, JsValue> {
        #[cfg(target_arch = "wasm32")]
        {
            let reader: web_sys::ReadableStreamDefaultReader = reader.unchecked_into();
            transport::read_from_stream(&reader).await
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = reader;
            Err(JsValue::from_str("WebTransport requires wasm32 target"))
        }
    }

    /// Write data to a writable stream.
    #[wasm_bindgen(js_name = "writeStream")]
    pub async fn write_stream(&self, writer: JsValue, data: &[u8]) -> Result<(), JsValue> {
        #[cfg(target_arch = "wasm32")]
        {
            let writer: web_sys::WritableStreamDefaultWriter = writer.unchecked_into();
            transport::write_to_stream(&writer, data).await
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (writer, data);
            Err(JsValue::from_str("WebTransport requires wasm32 target"))
        }
    }

    /// Check if connected.
    #[wasm_bindgen(js_name = "isConnected")]
    pub fn is_connected(&self) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            self.transport.is_some()
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            false
        }
    }

    /// Close the connection.
    pub fn close(&mut self) {
        #[cfg(target_arch = "wasm32")]
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
