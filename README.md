# Lumina WM — Pure-Rust Wayland Window Manager & HTML/CSS Desktop Shell

**Lumina WM** là một Wayland Window Manager (Compositor) kiêm Desktop Shell viết hoàn toàn bằng **Rust**, sử dụng **HTML5 + CSS3** để dựng toàn bộ giao diện (Topbar, Dock, App Launcher, Quick Settings, Notifications, và cả Server-Side Window Decorations) mà **không dùng WebKit, Blink hay Electron**.

## Kiến trúc 10 Crates (`crates/*`)

1. **`crates/wm-proto`**: Kiểu dữ liệu dùng chung (`StateValue`, `StatePatch`, `ActionCommand`, `ComputedStyle`, `LayoutBox`, `IpcCommand`) và các Trait nền tảng.
2. **`crates/reactive-state`**: Cây trạng thái phân cấp `StateStore` và trình biên dịch biểu thức `{{ expr | filter }}` (`default`, `truncate`, `round`, `upper`, `lower`, `percent`).
3. **`crates/sys-providers`**: Các bộ thu thập dữ liệu hệ thống Linux thời gian thực (`/proc/stat` CPU, `/proc/meminfo` RAM, `/sys/class/power_supply` Battery, `wpctl` PipeWire Audio, `/proc/net/dev` Network, `.desktop` App Scanner & Fuzzy Search, Notifications).
4. **`crates/html-dom`**: Bộ phân tích HTML5/XHTML không cần WebKit, Arena DOM, directives phản ứng (`data-for` keyed diffing, `data-if`, `data-show`, `:class.*`, `:style.*`, `:attr`, `{{...}}`) và lan truyền sự kiện (`on-click`, `on-scroll-*`, `on-input`, `on-keydown-*`).
5. **`crates/css-style`**: Bộ phân tích CSS3, khớp bộ chọn (`#id`, `.class`, `[attr]`, `:hover`, `:active`, `:focus`, combinators ` `, `>`), tính độ ưu tiên W3C Specificity Cascade, và biến động `var(--*)`.
6. **`crates/layout-engine`**: Động cơ tính toán bố cục **Taffy** (Flexbox, CSS Grid, Block, Absolute) và Hit-Testing chuột theo thứ tự `z-index`.
7. **`crates/renderer`**: Bộ kết xuất đồ họa vector 2D SIMD bằng `tiny-skia` + `ab_glyph` (bo góc `border-radius`, đổ bóng `box-shadow`, dải màu `linear-gradient`, icon vector/SVG, chữ TrueType + font dự phòng tích hợp sẵn, và khung cửa sổ Server-Side Decorations).
8. **`crates/wm-core`**: Máy trạng thái Window Manager hỗ trợ **5 chế độ bố cục** (`Tiling` Master-Stack, `Scrolling` cuộn cột kiểu Niri, `Bsp` chia đôi liên tiếp, `Monocle`, `Floating`), 9 Workspaces, và tích hợp Wayland Compositor (`WaylandCompositorHost`).
9. **`crates/ipc-server`**: Máy chủ & khách Unix Domain Socket giao thức NDJSON kèm bộ theo dõi file `AssetWatcher` (`notify`) để Hot-Reload HTML/CSS tức thì.
10. **`crates/wm-cli`**: Cung cấp 2 binary thực thi:
    - **`lumina-wm`**: Khởi chạy Wayland Compositor & HTML/CSS Shell Daemon.
    - **`luminactl`**: Công cụ CLI điều khiển WM, truy vấn DOM (`dom query`, `dom tree`), kiểm tra CSS (`dom style`), đổi biến CSS trực tiếp (`css set-var`), và bật/tắt widget.

## Cách chạy & Kiểm thử

```bash
# Chạy toàn bộ 41 unit & integration tests trên 10 crates
cargo test --workspace

# Biên dịch lumina-wm và luminactl
cargo build --bins

# Khởi chạy daemon và xuất ảnh chụp màn hình 1920x1080 với theme Catppuccin hoặc Cyberpunk
./target/debug/lumina-wm --theme catppuccin --dispatch "shell:open-widget('launcher')" --screenshot desktop.bmp --headless-frames 1

# Điều khiển và truy vấn DOM/CSS thời gian thực qua luminactl
./target/debug/luminactl wm workspaces
./target/debug/luminactl wm windows
./target/debug/luminactl dispatch "wm:set-layout-mode(scrolling)"
./target/debug/luminactl dom query ".ws-btn.active" --surface topbar
./target/debug/luminactl dom style "#topbar" --surface panel
./target/debug/luminactl css set-var --accent "#f38ba8"
./target/debug/luminactl widget toggle launcher
```
