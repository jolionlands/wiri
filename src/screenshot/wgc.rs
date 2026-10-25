//! One-shot WGC capture; the caller owns the COM apartment.
//!
//! windows 0.58 features (in addition to Win32_Foundation): Foundation,
//! Graphics_Capture, Graphics_DirectX_Direct3D11, Win32_Graphics_Direct3D,
//! Win32_Graphics_Direct3D11, Win32_Graphics_Dxgi_Common,
//! Win32_System_WinRT_Direct3D11, Win32_System_WinRT_Graphics_Capture.
//! Their parent namespace features are enabled transitively.

use super::Image;
use anyhow::{ensure, Context, Result};
use std::sync::mpsc::sync_channel;
use std::time::{Duration, Instant};
use windows::core::{factory, IInspectable, Interface};
use windows::Foundation::{EventRegistrationToken, TypedEventHandler};
use windows::Graphics::Capture::{
    Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem,
    GraphicsCaptureSession, IDirect3D11CaptureFramePoolStatics2, IGraphicsCaptureSessionStatics,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{HMODULE, HWND, RPC_E_CHANGED_MODE};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::{Common::DXGI_FORMAT_B8G8R8A8_UNORM, IDXGIDevice};
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::System::WinRT::{
    RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED, RO_INIT_SINGLETHREADED,
};

// Leave room in the 1500 ms IPC budget for readback and the caller's fallback/encoding.
// Setup time is deducted too; there is never a second frame wait or a polling loop.
const FRAME_BUDGET: Duration = Duration::from_millis(650);

struct WinRtScope;
impl WinRtScope {
    fn new() -> Result<Self> {
        let initialized = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
        match initialized {
            Err(error) if error.code() == RPC_E_CHANGED_MODE => {
                // CoInitializeEx may already have put the caller in an STA. Do not
                // change its apartment; initialize WinRT in that same apartment.
                unsafe { RoInitialize(RO_INIT_SINGLETHREADED) }
            }
            other => other,
        }.context("initializing WinRT for capture")?;
        // Both S_OK and S_FALSE take a reference that must be balanced.
        Ok(Self)
    }
}
impl Drop for WinRtScope {
    fn drop(&mut self) {
        unsafe { RoUninitialize() };
    }
}

// Install the guard immediately after pool creation, before any other fallible work.
// The handler captures only a channel sender, never this pool or the session.
struct CaptureScope {
    pool: Direct3D11CaptureFramePool,
    session: Option<GraphicsCaptureSession>,
    handler: Option<EventRegistrationToken>,
}
impl Drop for CaptureScope {
    fn drop(&mut self) {
        if let Some(token) = self.handler.take() {
            let _ = self.pool.RemoveFrameArrived(token);
        }
        if let Some(session) = self.session.take() {
            let _ = session.Close();
        }
        let _ = self.pool.Close();
    }
}

struct FrameScope(Direct3D11CaptureFrame);
impl Drop for FrameScope {
    fn drop(&mut self) {
        let _ = self.0.Close();
    }
}

struct UnmapScope<'a> {
    context: &'a ID3D11DeviceContext,
    texture: &'a ID3D11Texture2D,
}
impl Drop for UnmapScope<'_> {
    fn drop(&mut self) {
        unsafe { self.context.Unmap(self.texture, 0) };
    }
}

/// Capture one window as tightly packed, top-down, opaque BGRA8 pixels.
/// No dispatcher/message pump, application worker, timer, or cached device is needed.
pub fn capture(hwnd: HWND) -> Result<Image> {
    let started = Instant::now();
    ensure!(!hwnd.is_invalid(), "cannot capture an invalid window");
    tracing::debug!("WGC: initializing request-local WinRT factories");
    let _winrt = WinRtScope::new()?;
    // Generated static helpers cache agile factories for the process lifetime. That
    // outlives our request-local COM/WinRT apartment and can leave a stale factory on
    // the next capture. Use uncached factories and release them before RoUninitialize.
    let session_factory = factory::<GraphicsCaptureSession, IGraphicsCaptureSessionStatics>()
        .context("getting capture session statics")?;
    let mut supported = false;
    unsafe { (session_factory.vtable().IsSupported)(session_factory.as_raw(), &mut supported) }
        .ok().context("checking WGC support")?;
    ensure!(supported, "Windows.Graphics.Capture is unsupported");

    let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
        .context("getting GraphicsCaptureItem interop")?;
    let item: GraphicsCaptureItem = unsafe { interop.CreateForWindow(hwnd) }
        .context("creating capture item for window")?;
    let size = item.Size().context("getting capture item size")?;
    image_layout(size.Width, size.Height)?;

    let (device, context) = create_device()?;
    let dxgi: IDXGIDevice = device.cast().context("getting DXGI device")?;
    let direct3d: IDirect3DDevice = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }
        .context("wrapping D3D11 device for WinRT")?
        .cast().context("getting WinRT Direct3D device")?;
    let pool_factory = factory::<Direct3D11CaptureFramePool, IDirect3D11CaptureFramePoolStatics2>()
        .context("getting capture frame pool statics")?;
    let mut raw_pool = std::ptr::null_mut();
    unsafe {
        (pool_factory.vtable().CreateFreeThreaded)(pool_factory.as_raw(), direct3d.as_raw(),
            DirectXPixelFormat::B8G8R8A8UIntNormalized, 1, size, &mut raw_pool)
    }.ok().context("creating free-threaded capture frame pool")?;
    ensure!(!raw_pool.is_null(), "WGC returned a null capture pool");
    // Successful ABI result transfers one reference; scope closes/releases it below.
    let pool = unsafe { Direct3D11CaptureFramePool::from_raw(raw_pool) };
    let mut scope = CaptureScope { pool, session: None, handler: None };
    scope.session = Some(scope.pool.CreateCaptureSession(&item)
        .context("creating capture session")?);
    let session = scope.session.as_ref().context("capture session was not created")?;
    // Cursor exclusion is required; border suppression is only best effort (OS policy).
    session.SetIsCursorCaptureEnabled(false).context("disabling capture cursor")?;
    let _ = session.SetIsBorderRequired(false);

    let (tx, rx) = sync_channel(1);
    let handler = TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(move |_, _| {
        // Never block an API callback, including during teardown or after timeout.
        let _ = tx.try_send(());
        Ok(())
    });
    scope.handler = Some(scope.pool.FrameArrived(&handler)
        .context("subscribing to captured frames")?);
    ensure!(started.elapsed() < FRAME_BUDGET, "WGC setup exceeded the frame budget");
    session.StartCapture().context("starting capture session")?;
    rx.recv_timeout(FRAME_BUDGET.saturating_sub(started.elapsed()))
        .context("waiting for WGC frame (650 ms budget)")?;
    ensure!(started.elapsed() < FRAME_BUDGET, "WGC frame arrived after the frame budget");

    // This is the sole dequeue, prompted by FrameArrived, not a TryGetNextFrame poll.
    let frame = FrameScope(scope.pool.TryGetNextFrame().context("getting captured frame")?);
    read_frame(&device, &context, &frame.0)
}

fn create_device() -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    let mut device = None;
    let mut context = None;
    unsafe {
        D3D11CreateDevice(
            None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION,
            Some(&mut device), None, Some(&mut context),
        )
    }.context("creating capture D3D11 device")?;
    Ok((device.context("D3D11 device was not returned")?,
        context.context("D3D11 immediate context was not returned")?))
}

fn image_layout(width: i32, height: i32) -> Result<(usize, usize)> {
    ensure!(width > 0 && height > 0, "capture has nonpositive dimensions");
    let row = (width as usize).checked_mul(4).context("capture row is too large")?;
    let len = row.checked_mul(height as usize).context("capture is too large")?;
    ensure!(len <= u32::MAX as usize && len <= isize::MAX as usize,
        "capture exceeds image buffer limits");
    Ok((row, len))
}

fn read_frame(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    frame: &Direct3D11CaptureFrame,
) -> Result<Image> {
    let size = frame.ContentSize().context("getting captured content size")?;
    let (row_bytes, len) = image_layout(size.Width, size.Height)?;
    let surface = frame.Surface().context("getting captured surface")?;
    let access: IDirect3DDxgiInterfaceAccess = surface.cast()
        .context("getting captured surface DXGI access")?;
    let source: ID3D11Texture2D = unsafe { access.GetInterface() }
        .context("getting captured D3D11 texture")?;
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe { source.GetDesc(&mut desc) };
    ensure!(desc.Format == DXGI_FORMAT_B8G8R8A8_UNORM, "capture surface is not BGRA8");
    // A resize can race this one-shot capture. Never read outside the actual texture
    // or return a silently truncated frame; let the caller's fallback handle it.
    ensure!(size.Width as u32 <= desc.Width && size.Height as u32 <= desc.Height,
        "window resized beyond the capture surface");
    ensure!(desc.MipLevels == 1 && desc.ArraySize == 1 && desc.SampleDesc.Count == 1,
        "unexpected capture texture layout");

    let staging_desc = D3D11_TEXTURE2D_DESC {
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
        ..desc
    };
    let mut staging = None;
    unsafe { device.CreateTexture2D(&staging_desc, None, Some(&mut staging)) }
        .context("creating capture readback texture")?;
    let staging = staging.context("D3D11 staging texture was not returned")?;
    let mut pixels = vec![0; len];
    unsafe { context.CopyResource(&staging, &source) };
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    unsafe { context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)) }
        .context("mapping capture readback texture")?;
    let _unmap = UnmapScope { context, texture: &staging };
    ensure!(!mapped.pData.is_null(), "mapped capture texture has no data");
    let pitch = mapped.RowPitch as usize;
    ensure!(pitch >= row_bytes, "mapped capture row pitch is too small");
    let extent = (size.Height as usize - 1).checked_mul(pitch)
        .and_then(|offset| offset.checked_add(row_bytes))
        .context("mapped capture stride overflow")?;
    ensure!(extent <= isize::MAX as usize, "mapped capture exceeds pointer limits");

    for (y, row) in pixels.chunks_exact_mut(row_bytes).enumerate() {
        // Content dimensions fit desc, and each row fits RowPitch. Copy only the
        // pixels, never padding (not even the last row's potentially absent padding).
        unsafe {
            std::ptr::copy_nonoverlapping(
                mapped.pData.cast::<u8>().add(y * pitch), row.as_mut_ptr(), row_bytes,
            );
        }
        for pixel in row.chunks_exact_mut(4) { pixel[3] = 255; }
    }
    Ok(Image { width: size.Width as u32, height: size.Height as u32, pixels })
}
