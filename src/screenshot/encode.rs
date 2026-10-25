//! PNG via the OS WIC encoder; no image/PNG crate or resident codec objects.
use super::Image;
use anyhow::{ensure, Result};
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::StructuredStorage::{CreateStreamOnHGlobal, GetHGlobalFromStream};
use windows::Win32::System::Com::*;
use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};

pub struct ComScope(bool);
impl ComScope {
    pub fn new() -> Result<Self> {
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        // A caller on an already-initialized STA can still use WIC and WinRT here.
        if hr == windows::Win32::Foundation::RPC_E_CHANGED_MODE {
            return Ok(Self(false));
        }
        hr.ok()?;
        Ok(Self(true))
    }
}
impl Drop for ComScope {
    fn drop(&mut self) {
        if self.0 {
            unsafe {
                CoUninitialize();
            }
        }
    }
}

pub fn png(image: &Image) -> Result<Vec<u8>> {
    unsafe {
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let stream = CreateStreamOnHGlobal(None, true)?;
        let encoder = factory.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
        encoder.Initialize(&stream, WICBitmapEncoderNoCache)?;
        let mut frame = None;
        encoder.CreateNewFrame(&mut frame, std::ptr::null_mut())?;
        let frame = frame.unwrap();
        frame.Initialize(None)?;
        frame.SetSize(image.width, image.height)?;
        let mut format = GUID_WICPixelFormat32bppBGRA;
        frame.SetPixelFormat(&mut format)?;
        ensure!(
            format == GUID_WICPixelFormat32bppBGRA,
            "WIC PNG rejected BGRA pixels"
        );
        frame.WritePixels(image.height, image.width * 4, &image.pixels)?;
        frame.Commit()?;
        encoder.Commit()?;
        let mut stat = STATSTG::default();
        stream.Stat(&mut stat, STATFLAG_NONAME)?;
        let memory = GetHGlobalFromStream(&stream)?;
        let ptr = GlobalLock(memory);
        ensure!(!ptr.is_null(), "cannot read WIC PNG stream");
        let bytes = std::slice::from_raw_parts(ptr.cast::<u8>(), stat.cbSize as usize).to_vec();
        let _ = GlobalUnlock(memory);
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wic_writes_png_header_and_dimensions() {
        let _com = ComScope::new().unwrap();
        let bytes = png(&Image {
            width: 2,
            height: 3,
            pixels: vec![255; 24],
        })
        .unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&bytes[16..24], &[0, 0, 0, 2, 0, 0, 0, 3]);
    }
}
