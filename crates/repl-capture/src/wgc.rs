// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors

//! 基于 Windows Graphics Capture 的按需截图。
//!
//! 帧数由尺子进程负责，本模块**只在暂停态下按需抓单张图**：开局像素触发、
//! 编队匹配、每次部署前刷新部署栏。所以这里追求的是"随时能抓到一张正确的图"，
//! 而不是高帧率吞吐。
//!
//! 会话在首次使用时建立并保持，避免每次抓图都付 D3D 设备创建的开销
//! （约 50ms）；[`WindowCapturer::close`] 或 `Drop` 时释放。

use repl_core::Rect;

use crate::{window::GameWindow, CaptureError};

/// 一张 BGRA8 截图（客户区，已裁掉窗口边框和标题栏）。
#[derive(Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// BGRA8，行优先，无额外行填充（已按 `width * 4` 紧凑排布）。
    pub pixels: Vec<u8>,
}

impl Frame {
    pub const BYTES_PER_PIXEL: usize = 4;

    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Self {
        debug_assert_eq!(pixels.len(), width as usize * height as usize * 4);
        Self {
            width,
            height,
            pixels,
        }
    }

    /// 取 (x, y) 的 BGRA。越界返回 `None`。
    #[inline]
    pub fn pixel(&self, x: i32, y: i32) -> Option<[u8; 4]> {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return None;
        }
        let offset = (y as usize * self.width as usize + x as usize) * Self::BYTES_PER_PIXEL;
        Some([
            self.pixels[offset],
            self.pixels[offset + 1],
            self.pixels[offset + 2],
            self.pixels[offset + 3],
        ])
    }

    /// 取 (x, y) 的 RGB（丢弃 alpha）。
    #[inline]
    pub fn rgb(&self, x: i32, y: i32) -> Option<[u8; 3]> {
        self.pixel(x, y).map(|[b, g, r, _]| [r, g, b])
    }

    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.width as i32, self.height as i32)
    }

    /// 裁出 `src` 区域并重采样到 `dst_width × dst_height`（双线性）。
    ///
    /// **识别前必须做这一步。** MAA 的全部模板和 ROI 常量都是在 1280×720 下标定的，
    /// 而归一化互相关没有尺度不变性 —— 直接拿 10×11 的旗标模板去 2560×1440 的图上搜，
    /// 一个都匹配不到。MAA 自己也是先把截图缩到 1280×720 再识别的。
    ///
    /// 顺带的好处：编队绑定存下来的头像裁图尺寸与窗口分辨率无关，换分辨率也能复用。
    pub fn resample(&self, src: Rect, dst_width: u32, dst_height: u32) -> Self {
        let src = src.clamped(self.width as i32, self.height as i32);
        if src.is_empty() || dst_width == 0 || dst_height == 0 {
            return Self::new(0, 0, Vec::new());
        }
        let mut out = vec![0u8; dst_width as usize * dst_height as usize * 4];
        // 半像素偏移：把目标像素的中心映射回源图，避免整体偏半个像素。
        let sx = f64::from(src.width) / f64::from(dst_width);
        let sy = f64::from(src.height) / f64::from(dst_height);

        for dy in 0..dst_height {
            let fy = (f64::from(dy) + 0.5) * sy - 0.5 + f64::from(src.y);
            let y0 = fy.floor();
            let wy = fy - y0;
            let y0 = (y0 as i32).clamp(src.y, src.bottom() - 1);
            let y1 = (y0 + 1).min(src.bottom() - 1);

            for dx in 0..dst_width {
                let fx = (f64::from(dx) + 0.5) * sx - 0.5 + f64::from(src.x);
                let x0 = fx.floor();
                let wx = fx - x0;
                let x0 = (x0 as i32).clamp(src.x, src.right() - 1);
                let x1 = (x0 + 1).min(src.right() - 1);

                let i00 = self.index(x0, y0);
                let i10 = self.index(x1, y0);
                let i01 = self.index(x0, y1);
                let i11 = self.index(x1, y1);
                let dst = (dy as usize * dst_width as usize + dx as usize) * 4;

                for c in 0..4 {
                    let top = f64::from(self.pixels[i00 + c]) * (1.0 - wx)
                        + f64::from(self.pixels[i10 + c]) * wx;
                    let bottom = f64::from(self.pixels[i01 + c]) * (1.0 - wx)
                        + f64::from(self.pixels[i11 + c]) * wx;
                    out[dst + c] = (top * (1.0 - wy) + bottom * wy).round().clamp(0.0, 255.0) as u8;
                }
            }
        }
        Self::new(dst_width, dst_height, out)
    }

    #[inline]
    fn index(&self, x: i32, y: i32) -> usize {
        let x = x.clamp(0, self.width as i32 - 1) as usize;
        let y = y.clamp(0, self.height as i32 - 1) as usize;
        (y * self.width as usize + x) * Self::BYTES_PER_PIXEL
    }
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Frame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("bytes", &self.pixels.len())
            .finish()
    }
}

#[cfg(windows)]
pub use imp::WindowCapturer;

#[cfg(windows)]
mod imp {
    use super::*;

    use std::time::{Duration, Instant};

    use windows::core::Interface;
    use windows::Graphics::Capture::{
        Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
    };
    use windows::Graphics::DirectX::DirectXPixelFormat;
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
    use windows::Win32::Graphics::Direct3D11::{
        D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
        D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE,
        D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    };
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
    use windows::Win32::Graphics::Dxgi::IDXGIDevice;
    use windows::Win32::System::WinRT::Direct3D11::{
        CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
    };
    use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;

    /// 抓一帧的等待上限。WGC 只在画面有变化时产帧，游戏暂停时可能好几十毫秒才来一帧。
    const FRAME_TIMEOUT: Duration = Duration::from_millis(1500);
    /// 轮询 `TryGetNextFrame` 的间隔。
    const POLL: Duration = Duration::from_millis(2);
    /// 帧池缓冲数。1 就够 —— 我们只要最新的一帧。
    const BUFFERS: i32 = 1;

    /// 绑定到某个窗口的截图会话。
    pub struct WindowCapturer {
        window: GameWindow,
        device: ID3D11Device,
        context: ID3D11DeviceContext,
        _item: GraphicsCaptureItem,
        pool: Direct3D11CaptureFramePool,
        session: GraphicsCaptureSession,
        /// 会话建立时的窗口尺寸。窗口改变大小后需要重建会话。
        item_size: (i32, i32),
    }

    impl WindowCapturer {
        /// 当前系统是否支持 WGC（Windows 10 1803+）。
        pub fn is_supported() -> bool {
            GraphicsCaptureSession::IsSupported().unwrap_or(false)
        }

        pub fn new(window: GameWindow) -> Result<Self, CaptureError> {
            if !Self::is_supported() {
                return Err(CaptureError::Unsupported);
            }
            let hwnd = HWND(window.raw_handle() as *mut core::ffi::c_void);

            // 1. D3D11 设备（BGRA_SUPPORT 是 WGC 的硬性要求）
            let mut device: Option<ID3D11Device> = None;
            let mut context: Option<ID3D11DeviceContext> = None;
            // SAFETY: 输出参数都是合法的 Option 槽位。
            unsafe {
                D3D11CreateDevice(
                    None,
                    D3D_DRIVER_TYPE_HARDWARE,
                    None,
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    None,
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    Some(&mut context),
                )
                .map_err(|e| CaptureError::Backend(format!("D3D11CreateDevice 失败：{e}")))?;
            }
            let device = device.ok_or_else(|| CaptureError::Backend("D3D11 设备为空".into()))?;
            let context =
                context.ok_or_else(|| CaptureError::Backend("D3D11 上下文为空".into()))?;

            // 2. WinRT 侧的 IDirect3DDevice
            let dxgi: IDXGIDevice = device
                .cast()
                .map_err(|e| CaptureError::Backend(format!("取 IDXGIDevice 失败：{e}")))?;
            // SAFETY: dxgi 是刚从 D3D11 设备 cast 出来的有效接口。
            let winrt_device = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }
                .map_err(|e| CaptureError::Backend(format!("创建 WinRT D3D 设备失败：{e}")))?;
            let winrt_device: windows::Graphics::DirectX::Direct3D11::IDirect3DDevice =
                winrt_device.cast().map_err(|e| {
                    CaptureError::Backend(format!("cast IDirect3DDevice 失败：{e}"))
                })?;

            // 3. 从 HWND 建 GraphicsCaptureItem
            let interop: IGraphicsCaptureItemInterop =
                windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
                    .map_err(|e| CaptureError::Backend(format!("取 capture interop 失败：{e}")))?;
            // SAFETY: hwnd 有效；CreateForWindow 失败会返回 Err。
            let item: GraphicsCaptureItem = unsafe { interop.CreateForWindow(hwnd) }
                .map_err(|e| CaptureError::Backend(format!("为窗口创建捕获项失败：{e}")))?;
            let size = item
                .Size()
                .map_err(|e| CaptureError::Backend(format!("取捕获项尺寸失败：{e}")))?;

            // 4. 帧池 + 会话
            let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
                &winrt_device,
                DirectXPixelFormat::B8G8R8A8UIntNormalized,
                BUFFERS,
                size,
            )
            .map_err(|e| CaptureError::Backend(format!("创建帧池失败：{e}")))?;
            let session = pool
                .CreateCaptureSession(&item)
                .map_err(|e| CaptureError::Backend(format!("创建捕获会话失败：{e}")))?;

            // 我们从不移动鼠标，但仍然显式关掉光标捕获 —— 万一用户手动动了鼠标，
            // 光标也不该混进用于识别的图里。
            let _ = session.SetIsCursorCaptureEnabled(false);
            // Win11 才有；关掉那圈黄色捕获边框，否则会污染边缘的识别区域。
            let _ = session.SetIsBorderRequired(false);

            session
                .StartCapture()
                .map_err(|e| CaptureError::Backend(format!("启动捕获失败：{e}")))?;

            Ok(Self {
                window,
                device,
                context,
                _item: item,
                pool,
                session,
                item_size: (size.Width, size.Height),
            })
        }

        pub fn window(&self) -> GameWindow {
            self.window
        }

        /// 抓一张客户区截图。
        ///
        /// 窗口尺寸变了会返回 [`CaptureError::WindowResized`]，调用方应当重建
        /// `WindowCapturer` 并重算坐标映射 —— 尺寸变了之后所有几何常量都作废，
        /// 硬撑着继续跑只会点错地方。
        pub fn grab(&mut self) -> Result<Frame, CaptureError> {
            let geometry = self.window.geometry()?;
            let crop = self.client_crop_offset()?;

            let deadline = Instant::now() + FRAME_TIMEOUT;
            loop {
                // 没有新帧时 TryGetNextFrame 返回错误，这不是故障 —— 游戏暂停时
                // 画面不变，WGC 就不产帧，等下一轮即可。
                if let Ok(frame) = self.pool.TryGetNextFrame() {
                    let size = frame
                        .ContentSize()
                        .map_err(|e| CaptureError::Backend(format!("取帧尺寸失败：{e}")))?;
                    if (size.Width, size.Height) != self.item_size {
                        return Err(CaptureError::WindowResized);
                    }
                    let surface = frame
                        .Surface()
                        .map_err(|e| CaptureError::Backend(format!("取帧表面失败：{e}")))?;
                    let access: IDirect3DDxgiInterfaceAccess = surface
                        .cast()
                        .map_err(|e| CaptureError::Backend(format!("cast DXGI 访问器失败：{e}")))?;
                    // SAFETY: access 来自有效的捕获帧表面。
                    let texture: ID3D11Texture2D = unsafe { access.GetInterface() }
                        .map_err(|e| CaptureError::Backend(format!("取纹理失败：{e}")))?;
                    return self.read_back(&texture, geometry.width, geometry.height, crop);
                }
                if Instant::now() >= deadline {
                    return Err(CaptureError::FrameTimeout);
                }
                std::thread::sleep(POLL);
            }
        }

        /// 客户区左上角相对于 WGC 捕获纹理左上角的偏移。
        ///
        /// WGC 抓的是 DWM 合成后的整个窗口（含标题栏和边框），而我们所有的几何常量
        /// 都以客户区为基准，所以必须先裁掉。用 `DWMWA_EXTENDED_FRAME_BOUNDS` 而不是
        /// `GetWindowRect`：后者在 Win10+ 上包含不可见的阴影边距，会算出错误的偏移。
        fn client_crop_offset(&self) -> Result<(i32, i32), CaptureError> {
            let hwnd = HWND(self.window.raw_handle() as *mut core::ffi::c_void);
            let mut bounds = RECT::default();
            // SAFETY: 输出缓冲区大小与属性匹配。
            unsafe {
                DwmGetWindowAttribute(
                    hwnd,
                    DWMWA_EXTENDED_FRAME_BOUNDS,
                    &mut bounds as *mut RECT as *mut core::ffi::c_void,
                    std::mem::size_of::<RECT>() as u32,
                )
                .map_err(|e| CaptureError::Backend(format!("DwmGetWindowAttribute 失败：{e}")))?;
            }
            let geometry = self.window.geometry()?;
            Ok((
                geometry.origin.x - bounds.left,
                geometry.origin.y - bounds.top,
            ))
        }

        /// GPU 纹理 → CPU 内存，顺便裁剪出客户区。
        fn read_back(
            &self,
            texture: &ID3D11Texture2D,
            out_width: u32,
            out_height: u32,
            (crop_x, crop_y): (i32, i32),
        ) -> Result<Frame, CaptureError> {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            // SAFETY: texture 有效，desc 是合法的输出槽位。
            unsafe { texture.GetDesc(&mut desc) };

            // GPU 纹理不能直接被 CPU 读，要先拷到 staging 纹理。
            let staging_desc = D3D11_TEXTURE2D_DESC {
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
                ..desc
            };
            let mut staging: Option<ID3D11Texture2D> = None;
            // SAFETY: 描述符合法，输出参数有效。
            unsafe {
                self.device
                    .CreateTexture2D(&staging_desc, None, Some(&mut staging))
                    .map_err(|e| CaptureError::Backend(format!("创建 staging 纹理失败：{e}")))?;
            }
            let staging =
                staging.ok_or_else(|| CaptureError::Backend("staging 纹理为空".into()))?;

            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            // SAFETY: 两个纹理描述一致；Map/Unmap 成对调用。
            unsafe {
                self.context.CopyResource(&staging, texture);
                self.context
                    .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                    .map_err(|e| CaptureError::Backend(format!("Map staging 纹理失败：{e}")))?;
            }

            let result = (|| {
                let src_pitch = mapped.RowPitch as usize;
                if mapped.pData.is_null() {
                    return Err(CaptureError::Backend("Map 返回空指针".into()));
                }
                // 裁剪区必须落在纹理内，否则说明窗口几何和捕获尺寸对不上了。
                if crop_x < 0
                    || crop_y < 0
                    || crop_x as u32 + out_width > desc.Width
                    || crop_y as u32 + out_height > desc.Height
                {
                    return Err(CaptureError::WindowResized);
                }
                let mut pixels = vec![0u8; out_width as usize * out_height as usize * 4];
                let row_bytes = out_width as usize * 4;
                for row in 0..out_height as usize {
                    let src_offset = (crop_y as usize + row) * src_pitch + crop_x as usize * 4;
                    // SAFETY: 上面已校验裁剪区在纹理范围内；每行读取 row_bytes 字节，
                    // 不会越过 src_pitch。
                    let src = unsafe { (mapped.pData as *const u8).add(src_offset) };
                    let dst = &mut pixels[row * row_bytes..(row + 1) * row_bytes];
                    // SAFETY: src 指向至少 row_bytes 字节的有效数据，dst 长度相同。
                    unsafe { std::ptr::copy_nonoverlapping(src, dst.as_mut_ptr(), row_bytes) };
                }
                Ok(Frame::new(out_width, out_height, pixels))
            })();

            // SAFETY: 与上面的 Map 配对，无论中间成功与否都要 Unmap。
            unsafe { self.context.Unmap(&staging, 0) };
            result
        }

        pub fn close(self) {
            drop(self);
        }
    }

    impl Drop for WindowCapturer {
        fn drop(&mut self) {
            let _ = self.session.Close();
            let _ = self.pool.Close();
        }
    }

    impl std::fmt::Debug for WindowCapturer {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("WindowCapturer")
                .field("window", &self.window)
                .field("item_size", &self.item_size)
                .finish()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, bgra: [u8; 4]) -> Frame {
        Frame::new(
            width,
            height,
            bgra.iter()
                .copied()
                .cycle()
                .take(width as usize * height as usize * 4)
                .collect(),
        )
    }

    #[test]
    fn pixel_accessors_respect_bounds() {
        let f = solid(4, 3, [1, 2, 3, 255]);
        assert_eq!(f.pixel(0, 0), Some([1, 2, 3, 255]));
        assert_eq!(f.pixel(3, 2), Some([1, 2, 3, 255]));
        assert_eq!(f.pixel(4, 2), None);
        assert_eq!(f.pixel(3, 3), None);
        assert_eq!(f.pixel(-1, 0), None);
    }

    #[test]
    fn rgb_reorders_bgra() {
        let f = solid(2, 2, [10, 20, 30, 255]); // B=10 G=20 R=30
        assert_eq!(f.rgb(0, 0), Some([30, 20, 10]));
    }

    #[test]
    fn bounds_matches_dimensions() {
        let f = solid(8, 5, [0, 0, 0, 255]);
        assert_eq!(f.bounds(), Rect::new(0, 0, 8, 5));
    }
}
