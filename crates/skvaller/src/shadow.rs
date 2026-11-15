//! The window's drop shadow on KDE Plasma, where the app draws the window's frame.
//!
//! KWin draws a window's shadow from its decoration, which a window without one
//! doesn't have. A client can hand KWin a shadow of its own instead, as Plasma's panels
//! and menus do (`org_kde_kwin_shadow` on Wayland, `_KDE_NET_WM_SHADOW` on X11), and
//! KWin draws it outside the window, where it takes no clicks. The shadow is Breeze's,
//! rendered as Breeze renders it ([`Texture::breeze`], after breezedecoration.cpp and
//! breezeboxshadowrenderer.cpp in Breeze 6.7): its default Large size and full
//! strength, black, half as strong while the window is inactive, around Breeze's
//! corners. The texture is cut into the eight tiles KWin asks for ([`Texture::tiles`]).

/// A shadow texture: an alpha mask of black, square, with the window's place at its
/// center pixel, which KWin stretches to the window's size.
#[derive(Debug)]
pub struct Texture {
    /// The side, in pixels.
    size: usize,
    /// The alpha, row by row.
    alpha: Vec<u8>,
    /// How far the shadow reaches outside the window: left, top, right and bottom.
    padding: [u32; 4],
}

/// One of Breeze's two shadows: an offset, a blur radius and an opacity.
struct Params {
    offset: (i32, i32),
    radius: u32,
    opacity: f64,
}

/// Breeze's Large shadow, its default: the whole shadow's offset, then its two parts.
const LARGE: ((i32, i32), Params, Params) = (
    (0, 12),
    Params {
        offset: (0, 0),
        radius: 48,
        opacity: 0.8,
    },
    Params {
        offset: (0, -6),
        radius: 24,
        opacity: 0.2,
    },
);

/// How far the shadow reaches under the window, past its edges (`Shadow_Overlap`).
const OVERLAP: i32 = 3;

impl Texture {
    /// Breeze's shadow at `strength` (1 for an active window, 0.5 for an inactive one),
    /// around corners of `corner_radius`.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss,
        reason = "pixel counts and positions of a few hundred"
    )]
    pub fn breeze(strength: f64, corner_radius: f64) -> Self {
        let (offset, first, second) = LARGE;
        let box_size = min_box_size(first.radius).max(min_box_size(second.radius));
        let size = texture_size(box_size, &first).max(texture_size(box_size, &second));
        let mut alpha = vec![0_u8; size * size];
        // The box's center in the canvas, as `QRect::center` has it.
        let center = (size - 1) / 2;
        for params in [&first, &second] {
            let shadow = render_one(box_size, params, strength);
            let span = shadow.size;
            // Centered on the box, then moved by the offset, the image lands on a half
            // pixel, rounded up so the shadow stays centered.
            let place = |o: i32| -> i64 {
                let at = center as f64 + f64::from(o) - span as f64 / 2.0;
                (at + 0.5).floor() as i64
            };
            let (left, top) = (place(params.offset.0), place(params.offset.1));
            for y in 0..span {
                for x in 0..span {
                    let (cx, cy) = (left + x as i64, top + y as i64);
                    if cx < 0 || cy < 0 || cx >= size as i64 || cy >= size as i64 {
                        continue;
                    }
                    let src = shadow.alpha[y * span + x];
                    let dst = &mut alpha[cy as usize * size + cx as usize];
                    // Source over, premultiplied black.
                    *dst = src.saturating_add(div_255(u32::from(*dst) * u32::from(255 - src)));
                }
            }
        }
        // The box sits at the canvas's center, in floating point.
        let canvas = size as f64;
        let box_left = (canvas - box_size as f64) / 2.0;
        let padding = [
            box_left - f64::from(OVERLAP) - f64::from(offset.0),
            box_left - f64::from(OVERLAP) - f64::from(offset.1),
            box_left - f64::from(OVERLAP) + f64::from(offset.0),
            box_left - f64::from(OVERLAP) + f64::from(offset.1),
        ];
        // Breeze cuts the window's place out, two pixels in from its edges, so the
        // shadow shows only around the window and under its rounded corners.
        let inner = (
            padding[0] + 2.0,
            padding[1] + 2.0,
            canvas - padding[2] - 2.0,
            canvas - padding[3] - 2.0,
        );
        cut_rounded_rect(&mut alpha, size, inner, corner_radius + 0.5);
        let padding = padding.map(|p| p as u32);
        Self {
            size,
            alpha,
            padding,
        }
    }

    /// The eight tiles around the center pixel, clockwise from the left as KWin's
    /// shadow protocol attaches them: left, top left, top, top right, right, bottom
    /// right, bottom and bottom left, each as its width, height and alpha.
    pub fn tiles(&self) -> [(usize, usize, Vec<u8>); 8] {
        let c = self.size / 2;
        let far = self.size - c - 1;
        let cut = |x: usize, y: usize, w: usize, h: usize| {
            let mut out = Vec::with_capacity(w * h);
            for row in y..y + h {
                out.extend_from_slice(&self.alpha[row * self.size + x..row * self.size + x + w]);
            }
            (w, h, out)
        };
        [
            cut(0, c, c, 1),
            cut(0, 0, c, c),
            cut(c, 0, 1, c),
            cut(c + 1, 0, far, c),
            cut(c + 1, c, far, 1),
            cut(c + 1, c + 1, far, far),
            cut(c, c + 1, 1, far),
            cut(0, c + 1, c, far),
        ]
    }
}

/// One shadow, before it joins the canvas: a square image of black at `params`'
/// opacity times `strength`.
struct Shadow {
    size: usize,
    alpha: Vec<u8>,
}

/// Breeze's `renderShadow`: the box, blurred, tinted.
fn render_one(box_size: usize, params: &Params, strength: f64) -> Shadow {
    let extent = blur_extent(params.radius);
    let size = box_size + 2 * extent;
    let mut alpha = vec![0_u8; size * size];
    // The box, centered: whole pixels, and its corners' radius under a tenth of a
    // pixel (Breeze passes a relative radius where Qt takes an absolute one).
    for y in extent..extent + box_size {
        alpha[y * size + extent..y * size + extent + box_size].fill(255);
    }
    // Symmetrical: Breeze blurs the top-left quadrant and mirrors it.
    blur(&mut alpha, size, params.radius);
    mirror(&mut alpha, size);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "an opacity, within 0 to 255"
    )]
    let tint = (params.opacity * strength * 255.0)
        .round()
        .clamp(0.0, 255.0) as u32;
    for a in &mut alpha {
        *a = div_255(u32::from(*a) * tint);
    }
    Shadow { size, alpha }
}

/// Qt's `qt_div_255`: `x / 255`, rounded.
fn div_255(x: u32) -> u8 {
    #[expect(clippy::cast_possible_truncation, reason = "x is at most 255 * 255")]
    let v = ((x + (x >> 8) + 0x80) >> 8) as u8;
    v
}

/// The blur's reach for a shadow `radius`: CSS's standard deviation, half the radius,
/// as SVG's three box blurs approximate it.
fn blur_extent(radius: u32) -> usize {
    let std_dev = f64::from(radius) * 0.5;
    let factor = (3.0 * (2.0 * std::f64::consts::PI).sqrt() / 4.0) * 1.5;
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a few dozen pixels"
    )]
    let r = (std_dev * factor + 0.5).floor() as usize;
    r.max(2)
}

fn min_box_size(radius: u32) -> usize {
    2 * blur_extent(radius) + 1
}

/// The side of a square canvas that holds one shadow, offset and all.
fn texture_size(box_size: usize, params: &Params) -> usize {
    let offset = params
        .offset
        .0
        .unsigned_abs()
        .max(params.offset.1.unsigned_abs());
    box_size + 2 * blur_extent(params.radius) + offset as usize
}

/// The three box blurs' left and right reach, for a shadow `radius`.
fn lobes(radius: u32) -> [(usize, usize); 3] {
    let blur = blur_extent(radius);
    let z = blur / 3;
    let (major, minor, last) = match blur % 3 {
        0 => (z, z, z),
        1 => (z + 1, z, z),
        _ => (z + 1, z, z + 1),
    };
    [(major, minor), (minor, major), (last, last)]
}

/// Breeze's `boxBlurRowAlpha`: one box blur along `src`, the ends repeated past it.
fn blur_line(src: &[u8], dst: &mut [u8], (left_lobe, right_lobe): (usize, usize)) {
    let width = src.len();
    let box_size = left_lobe + 1 + right_lobe;
    let reciprocal = (1_u64 << 24) / box_size as u64;
    let (first, last) = (u64::from(src[0]), u64::from(src[width - 1]));
    let mut sum = (box_size as u64).div_ceil(2) + first * left_lobe as u64;
    let mut right = 0;
    while right < box_size - left_lobe {
        sum += u64::from(src[right.min(width - 1)]);
        right += 1;
    }
    let value = |sum: u64| -> u8 {
        #[expect(clippy::cast_possible_truncation, reason = "an average of bytes")]
        let v = ((sum * reciprocal) >> 24) as u8;
        v
    };
    let (mut left, mut out) = (0, 0);
    while right < box_size && out < width {
        dst[out] = value(sum);
        sum = sum + u64::from(src[right.min(width - 1)]) - first;
        right += 1;
        out += 1;
    }
    while right < width {
        dst[out] = value(sum);
        sum = sum + u64::from(src[right]) - u64::from(src[left]);
        left += 1;
        right += 1;
        out += 1;
    }
    while out < width {
        dst[out] = value(sum);
        sum = sum + last - u64::from(src[left]);
        left += 1;
        out += 1;
    }
}

/// Breeze's `boxBlurAlpha` over the top-left quarter of a `size`-wide image, rounded
/// up as [`mirror`] copies it: three box blurs across, then three down.
fn blur(alpha: &mut [u8], size: usize, radius: u32) {
    if radius < 2 {
        return;
    }
    let half = size.div_ceil(2);
    let lobes = lobes(radius);
    let (mut a, mut b) = (vec![0_u8; half], vec![0_u8; half]);
    let mut line = vec![0_u8; half];
    for y in 0..half {
        line.copy_from_slice(&alpha[y * size..y * size + half]);
        blur_line(&line, &mut a, lobes[0]);
        blur_line(&a, &mut b, lobes[1]);
        blur_line(&b, &mut alpha[y * size..y * size + half], lobes[2]);
    }
    for x in 0..half {
        for (y, v) in line.iter_mut().enumerate() {
            *v = alpha[y * size + x];
        }
        blur_line(&line, &mut a, lobes[0]);
        blur_line(&a, &mut b, lobes[1]);
        blur_line(&b, &mut line, lobes[2]);
        for (y, v) in line.iter().enumerate() {
            alpha[y * size + x] = *v;
        }
    }
}

/// Breeze's `mirrorTopLeftQuadrant`: the top-left quarter copied to the others.
fn mirror(alpha: &mut [u8], size: usize) {
    let half = size.div_ceil(2);
    for y in 0..half {
        for x in 0..half {
            alpha[y * size + size - 1 - x] = alpha[y * size + x];
        }
    }
    // The middle row of an odd size stays as it is.
    for y in 0..size / 2 {
        let (top, bottom) = alpha.split_at_mut((size - y - 1) * size);
        bottom[..size].copy_from_slice(&top[y * size..y * size + size]);
    }
}

/// Clears what an antialiased rounded rectangle covers, as Qt's `DestinationOut`.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "pixel positions of a few hundred"
)]
fn cut_rounded_rect(alpha: &mut [u8], size: usize, (x0, y0, x1, y1): (f64, f64, f64, f64), r: f64) {
    const SAMPLES: u32 = 4;
    let inside = |x: f64, y: f64| {
        if x < x0 || x > x1 || y < y0 || y > y1 {
            return false;
        }
        let cx = x.clamp(x0 + r, x1 - r);
        let cy = y.clamp(y0 + r, y1 - r);
        (x - cx).powi(2) + (y - cy).powi(2) <= r * r
    };
    let (from_x, to_x, from_y, to_y) = (
        x0.floor().max(0.0) as usize,
        (x1.ceil() as usize).min(size),
        y0.floor().max(0.0) as usize,
        (y1.ceil() as usize).min(size),
    );
    for y in from_y..to_y {
        for x in from_x..to_x {
            let mut hits = 0;
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let px = x as f64 + (f64::from(sx) + 0.5) / f64::from(SAMPLES);
                    let py = y as f64 + (f64::from(sy) + 0.5) / f64::from(SAMPLES);
                    hits += u32::from(inside(px, py));
                }
            }
            let keep = SAMPLES * SAMPLES - hits;
            let a = &mut alpha[y * size + x];
            let kept = (u32::from(*a) * keep / (SAMPLES * SAMPLES)) as u8;
            *a = kept;
        }
    }
}

/// Hands the shadow to KWin: on Wayland through `org_kde_kwin_shadow`, on X11 through
/// the window's `_KDE_NET_WM_SHADOW` property.
pub mod kwin {
    use std::ffi::c_void;
    use std::os::fd::AsFd;
    use std::ptr::NonNull;
    use std::sync::OnceLock;

    use dioxus_native::winit::raw_window_handle::{RawDisplayHandle, RawWindowHandle};
    use dioxus_native::winit::window::Window;
    use wayland_backend::client::{Backend, ObjectId};
    use wayland_client::protocol::wl_buffer::WlBuffer;
    use wayland_client::protocol::wl_registry::{self, WlRegistry};
    use wayland_client::protocol::wl_shm::{self, WlShm};
    use wayland_client::protocol::wl_shm_pool::WlShmPool;
    use wayland_client::protocol::wl_surface::WlSurface;
    use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop};
    use wayland_protocols_plasma::shadow::client::org_kde_kwin_shadow::OrgKdeKwinShadow;
    use wayland_protocols_plasma::shadow::client::org_kde_kwin_shadow_manager::OrgKdeKwinShadowManager;

    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::{
        AtomEnum, ConnectionExt as _, CreateGCAux, ImageFormat, PropMode,
    };
    use x11rb::rust_connection::RustConnection;
    use x11rb::wrapper::ConnectionExt as _;

    use super::Texture;
    use crate::appearance::BREEZE_RADIUS;

    /// The shadows, active and inactive, around Breeze's corners, made once.
    fn textures() -> &'static [Texture; 2] {
        static TEXTURES: OnceLock<[Texture; 2]> = OnceLock::new();
        TEXTURES.get_or_init(|| {
            let radius = f64::from(BREEZE_RADIUS);
            [Texture::breeze(1.0, radius), Texture::breeze(0.5, radius)]
        })
    }

    /// The globals this needs, as the registry announces them.
    #[derive(Default)]
    struct Globals {
        manager: Option<(u32, u32)>,
        shm: Option<(u32, u32)>,
    }

    impl Dispatch<WlRegistry, ()> for Globals {
        fn event(
            state: &mut Self,
            _: &WlRegistry,
            event: wl_registry::Event,
            (): &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let wl_registry::Event::Global {
                name,
                interface,
                version,
            } = event
            {
                if interface == OrgKdeKwinShadowManager::interface().name {
                    state.manager = Some((name, version));
                } else if interface == WlShm::interface().name {
                    state.shm = Some((name, version));
                }
            }
        }
    }

    delegate_noop!(Globals: ignore WlShm);
    delegate_noop!(Globals: ignore WlBuffer);
    delegate_noop!(Globals: WlShmPool);
    delegate_noop!(Globals: OrgKdeKwinShadowManager);
    delegate_noop!(Globals: OrgKdeKwinShadow);

    /// A window's shadow at KWin, while the window lives.
    pub enum KwinShadow {
        // Boxed: once per window, and the two differ in size.
        Wayland(Box<WaylandShadow>),
        X11(Box<X11Shadow>),
    }

    impl KwinShadow {
        /// Sets up `window`'s shadow. `None` on a Wayland compositor without KWin's
        /// shadows, or a window on neither Wayland nor X11. On X11 the property is set
        /// whatever the window manager: only KWin reads it.
        pub fn new(window: &dyn Window) -> Option<Self> {
            let display = window
                .rwh_06_display_handle()
                .display_handle()
                .ok()?
                .as_raw();
            let handle = window.rwh_06_window_handle().window_handle().ok()?.as_raw();
            let mut shadow = match (display, handle) {
                (RawDisplayHandle::Wayland(d), RawWindowHandle::Wayland(w)) => {
                    Self::Wayland(Box::new(WaylandShadow::new(d.display, w.surface)?))
                }
                (_, RawWindowHandle::Xlib(w)) => {
                    Self::X11(Box::new(X11Shadow::new(u32::try_from(w.window).ok()?)?))
                }
                (_, RawWindowHandle::Xcb(w)) => {
                    Self::X11(Box::new(X11Shadow::new(w.window.get())?))
                }
                _ => return None,
            };
            shadow.set_active(window, window.has_focus());
            Some(shadow)
        }

        /// Shows the active window's shadow, or the inactive one's, as Breeze does.
        pub fn set_active(&mut self, window: &dyn Window, active: bool) {
            match self {
                Self::Wayland(s) => s.set_active(window, active),
                Self::X11(s) => s.set_active(active),
            }
        }
    }

    /// The shadow on Wayland: buffers of the tiles, attached to the window's surface.
    pub struct WaylandShadow {
        shadow: OrgKdeKwinShadow,
        /// The tiles of the active shadow, then the inactive one.
        buffers: [[WlBuffer; 8]; 2],
        /// Bound for this window, and let go with it.
        manager: OrgKdeKwinShadowManager,
        /// Left, top, right and bottom, the same at either strength.
        padding: [u32; 4],
        active: Option<bool>,
        connection: Connection,
        queue: EventQueue<Globals>,
    }

    impl WaylandShadow {
        /// The shadow of the window whose surface is `surface`, on winit's `display`.
        fn new(display: NonNull<c_void>, surface: NonNull<c_void>) -> Option<Self> {
            // SAFETY: winit's `wl_display`, which lives as long as the event loop, so
            // longer than any window. This connection only adds a queue of its own.
            let backend = unsafe { Backend::from_foreign_display(display.as_ptr().cast()) };
            let connection = Connection::from_backend(backend);
            // SAFETY: the window's `wl_surface`, alive while the window is. It is used
            // only from the window's own events, so while it lives.
            let surface_id =
                unsafe { ObjectId::from_ptr(WlSurface::interface(), surface.as_ptr().cast()) }
                    .ok()?;
            let surface = WlSurface::from_id(&connection, surface_id).ok()?;

            let mut queue = connection.new_event_queue::<Globals>();
            let qh = queue.handle();
            let registry = connection.display().get_registry(&qh, ());
            let mut globals = Globals::default();
            // A local round trip to the compositor, not the network.
            queue.roundtrip(&mut globals).ok()?;
            let (manager_name, manager_version) = globals.manager?;
            let (shm_name, shm_version) = globals.shm?;
            // Made once KWin's shadows are known to be there, and before binding
            // anything on winit's connection, so that a failure here leaves only the
            // registry there.
            let textures = textures();
            let tiles = textures.each_ref().map(Texture::tiles);
            let fd = rustix::fs::memfd_create("skvaller-shadow", rustix::fs::MemfdFlags::CLOEXEC)
                .ok()?;
            let mut file = std::fs::File::from(fd);
            let mut data =
                Vec::with_capacity(tiles.iter().flatten().map(|(w, h, _)| w * h * 4).sum());
            for (_, _, alpha) in tiles.iter().flatten() {
                // ARGB8888, little-endian, premultiplied black: only alpha.
                for a in alpha {
                    data.extend_from_slice(&[0, 0, 0, *a]);
                }
            }
            std::io::Write::write_all(&mut file, &data).ok()?;
            let pool_size = i32::try_from(data.len()).ok()?;
            let mut sizes = [[(0_i32, 0_i32); 8]; 2];
            for (out, tiles) in sizes.iter_mut().zip(&tiles) {
                for (size, (w, h, _)) in out.iter_mut().zip(tiles) {
                    *size = (i32::try_from(*w).ok()?, i32::try_from(*h).ok()?);
                }
            }

            let manager: OrgKdeKwinShadowManager =
                registry.bind(manager_name, manager_version.min(2), &qh, ());
            let shm: WlShm = registry.bind(shm_name, shm_version.min(2), &qh, ());
            let pool = shm.create_pool(file.as_fd(), pool_size, &qh, ());
            // In the order `data` holds the tiles.
            let mut offset = 0;
            let buffers = sizes.map(|tiles| {
                tiles.map(|(w, h)| {
                    let buffer =
                        pool.create_buffer(offset, w, h, w * 4, wl_shm::Format::Argb8888, &qh, ());
                    offset += w * h * 4;
                    buffer
                })
            });
            // The buffers keep the pool's memory, and need neither it nor `shm`.
            pool.destroy();
            if shm.version() >= 2 {
                shm.release();
            }
            let shadow = manager.create(&surface, &qh, ());
            let padding = textures[0].padding;
            Some(Self {
                shadow,
                buffers,
                manager,
                padding,
                active: None,
                connection,
                queue,
            })
        }

        fn set_active(&mut self, window: &dyn Window, active: bool) {
            if self.active == Some(active) {
                return;
            }
            self.active = Some(active);
            let which = usize::from(!active);
            let b = &self.buffers[which];
            let s = &self.shadow;
            s.attach_left(&b[0]);
            s.attach_top_left(&b[1]);
            s.attach_top(&b[2]);
            s.attach_top_right(&b[3]);
            s.attach_right(&b[4]);
            s.attach_bottom_right(&b[5]);
            s.attach_bottom(&b[6]);
            s.attach_bottom_left(&b[7]);
            let [left, top, right, bottom] = self.padding;
            s.set_left_offset(f64::from(left));
            s.set_top_offset(f64::from(top));
            s.set_right_offset(f64::from(right));
            s.set_bottom_offset(f64::from(bottom));
            s.commit();
            // The shadow takes effect with the surface's next commit: the next frame.
            window.request_redraw();
            let _ = self.connection.flush();
            // The buffers' releases, which nothing waits for.
            let _ = self.queue.dispatch_pending(&mut Globals::default());
        }
    }

    impl Drop for WaylandShadow {
        fn drop(&mut self) {
            if self.shadow.version() >= 2 {
                self.shadow.destroy();
            }
            for b in self.buffers.iter().flatten() {
                b.destroy();
            }
            if self.manager.version() >= 2 {
                self.manager.destroy();
            }
            let _ = self.connection.flush();
        }
    }

    /// The shadow on X11: pixmaps of the tiles, named in the window's
    /// `_KDE_NET_WM_SHADOW` property, on a connection of the app's own to the X server.
    pub struct X11Shadow {
        connection: RustConnection,
        window: u32,
        atom: u32,
        /// The tiles of the active shadow, then the inactive one, in the property's
        /// order: top, top right, right, bottom right, bottom, bottom left, left, top
        /// left.
        pixmaps: [[u32; 8]; 2],
        /// Top, right, bottom and left, as the property has them, the same at either
        /// strength.
        padding: [u32; 4],
        active: Option<bool>,
    }

    impl X11Shadow {
        /// The shadow of `window`, on the server `DISPLAY` names.
        fn new(window: u32) -> Option<Self> {
            let (connection, screen) = x11rb::connect(None).ok()?;
            let root = connection.setup().roots.get(screen)?.root;
            // Local round trips to the X server, not the network.
            let atom = connection
                .intern_atom(false, b"_KDE_NET_WM_SHADOW")
                .ok()?
                .reply()
                .ok()?
                .atom;
            let textures = textures();
            let mut pixmaps = [[0; 8]; 2];
            for (texture, ids) in textures.iter().zip(&mut pixmaps) {
                let tiles = texture.tiles();
                // From the tiles' order (left, then clockwise) to the property's (top).
                for (id, tile) in ids.iter_mut().zip([2, 3, 4, 5, 6, 7, 0, 1]) {
                    let (w, h, alpha) = &tiles[tile];
                    *id = pixmap(&connection, root, *w, *h, alpha)?;
                }
            }
            let [left, top, right, bottom] = textures[0].padding;
            let padding = [top, right, bottom, left];
            Some(Self {
                connection,
                window,
                atom,
                pixmaps,
                padding,
                active: None,
            })
        }

        fn set_active(&mut self, active: bool) {
            if self.active == Some(active) {
                return;
            }
            self.active = Some(active);
            let which = usize::from(!active);
            let mut value = [0_u32; 12];
            value[..8].copy_from_slice(&self.pixmaps[which]);
            value[8..].copy_from_slice(&self.padding);
            let set = self.connection.change_property32(
                PropMode::REPLACE,
                self.window,
                self.atom,
                AtomEnum::CARDINAL,
                &value,
            );
            if set.is_err() || self.connection.flush().is_err() {
                tracing::warn!("setting the window's shadow failed");
            }
        }
    }

    /// A 32-bit pixmap of a tile, `w` by `h`, premultiplied black of `alpha`.
    fn pixmap(
        connection: &RustConnection,
        root: u32,
        w: usize,
        h: usize,
        alpha: &[u8],
    ) -> Option<u32> {
        let (w, h) = (u16::try_from(w).ok()?, u16::try_from(h).ok()?);
        let id = connection.generate_id().ok()?;
        connection.create_pixmap(32, id, root, w, h).ok()?;
        let gc = connection.generate_id().ok()?;
        connection.create_gc(gc, id, &CreateGCAux::new()).ok()?;
        // BGRA in memory, as the server's little-endian ARGB: only alpha.
        let table = linear_light();
        let data: Vec<u8> = alpha
            .iter()
            .flat_map(|a| [0, 0, 0, table[usize::from(*a)]])
            .collect();
        connection
            .put_image(ImageFormat::Z_PIXMAP, id, gc, w, h, 0, 0, 0, 32, &data)
            .ok()?;
        connection.free_gc(gc).ok()?;
        Some(id)
    }

    /// [`as_in_linear_light`] for every alpha, made once.
    fn linear_light() -> &'static [u8; 256] {
        static TABLE: OnceLock<[u8; 256]> = OnceLock::new();
        TABLE.get_or_init(|| {
            std::array::from_fn(|a| as_in_linear_light(u8::try_from(a).unwrap_or(u8::MAX)))
        })
    }

    /// The alpha that darkens as `alpha` does when blended in linear light.
    ///
    /// KWin blends a decoration's shadow, and a Wayland client's, in linear light, but
    /// an X11 client's shadow pixmaps as they are, where the same alpha shows about
    /// twice as dark. Black at `a` over light `L` leaves `L·(1−a)` in linear light,
    /// which is the encoded value times `(1−a)^(1/2.2)`: this alpha.
    fn as_in_linear_light(alpha: u8) -> u8 {
        let a = f64::from(alpha) / 255.0;
        let encoded = 1.0 - (1.0 - a).powf(1.0 / 2.2);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "within 0 to 255"
        )]
        let v = (encoded * 255.0).round() as u8;
        v
    }

    impl Drop for X11Shadow {
        fn drop(&mut self) {
            // The window may be gone already; then so is its property.
            // The pixmaps go with the connection, which closes after this.
            let _ = self.connection.delete_property(self.window, self.atom);
            let _ = self.connection.flush();
        }
    }

    #[cfg(test)]
    mod tests {
        use super::as_in_linear_light;

        #[test]
        fn x11_shadows_are_lightened_as_linear_light_blends_them() {
            assert_eq!(as_in_linear_light(0), 0);
            assert_eq!(as_in_linear_light(255), 255);
            // Measured on the Steam Deck: 0.444 there darkens as 0.233 on Wayland.
            assert_eq!(as_in_linear_light(113), 60);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Texture, blur_extent, lobes};

    #[test]
    fn breezes_large_shadow_has_its_sizes() {
        assert_eq!(blur_extent(48), 68);
        assert_eq!(blur_extent(24), 34);
        assert_eq!(lobes(48), [(23, 22), (22, 23), (23, 23)]);
        assert_eq!(lobes(24), [(12, 11), (11, 12), (11, 11)]);
        let t = Texture::breeze(1.0, 5.0);
        assert_eq!(t.size, 273);
        assert_eq!(t.padding, [65, 53, 65, 77]);
    }

    #[test]
    fn the_shadow_is_darkest_near_the_window_and_gone_inside_it() {
        let t = Texture::breeze(1.0, 5.0);
        let at = |x: usize, y: usize| t.alpha[y * t.size + x];
        let c = t.size / 2;
        // Inside the window's place: cut out.
        assert_eq!(at(c, c), 0);
        // Fading out toward the texture's edges, deeper below than above: the window's
        // top edge is at 53, its bottom at 196.
        assert!(at(c, 50) > at(c, 20));
        assert!(at(c, 20) > at(c, 0));
        assert!(at(c, 206) > at(c, 43));
        // Symmetrical left and right.
        assert_eq!(at(40, c), at(t.size - 41, c));
        // Half as strong for an inactive window.
        let inactive = Texture::breeze(0.5, 5.0);
        assert!(inactive.alpha[c * t.size + 40] < at(40, c));
    }

    #[test]
    fn the_tiles_cover_the_texture() {
        let t = Texture::breeze(1.0, 5.0);
        let tiles = t.tiles();
        let area: usize = tiles.iter().map(|(w, h, _)| w * h).sum();
        assert_eq!(area + 1, t.size * t.size);
        for (w, h, alpha) in &tiles {
            assert_eq!(alpha.len(), w * h);
        }
    }
}
