#![windows_subsystem = "windows"]

use std::os::windows::process::CommandExt;
use std::process::Command;
use std::sync::mpsc::{self, Receiver, Sender};

use eframe::egui::{self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, RichText, Stroke, Vec2};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

const RED: Color32 = Color32::from_rgb(0xEC, 0x1B, 0x1B);
const BLACK: Color32 = Color32::BLACK;
const WHITE: Color32 = Color32::WHITE;
const GRAY: Color32 = Color32::from_rgb(0x70, 0x70, 0x70);
const LINE: Color32 = Color32::from_rgb(0xE4, 0xE4, 0xE4);
const R: u8 = 2;

const TASK_NAME: &str = "AdGuardDnsToggle";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

struct Provider {
    name: &'static str,
    note: &'static str,
    host: &'static str,
    v4: [&'static str; 2],
    v6: [&'static str; 2],
}

/// 첫 번째가 기본값(AdGuard).
const PROVIDERS: [Provider; 3] = [
    Provider {
        name: "AdGuard",
        note: "광고·추적 차단",
        host: "dns.adguard-dns.com",
        v4: ["94.140.14.14", "94.140.15.15"],
        v6: ["2a10:50c0::ad1:ff", "2a10:50c0::ad2:ff"],
    },
    Provider {
        name: "Cloudflare",
        note: "1.1.1.1 · 빠른 속도",
        host: "one.one.one.one",
        v4: ["1.1.1.1", "1.0.0.1"],
        v6: ["2606:4700:4700::1111", "2606:4700:4700::1001"],
    },
    Provider {
        name: "Google",
        note: "8.8.8.8 · 안정성",
        host: "dns.google",
        v4: ["8.8.8.8", "8.8.4.4"],
        v6: ["2001:4860:4860::8888", "2001:4860:4860::8844"],
    },
];

fn pref_path() -> Option<std::path::PathBuf> {
    std::env::var_os("APPDATA").map(|d| std::path::PathBuf::from(d).join("AdBlocker-thinkgood").join("provider.txt"))
}

fn load_selected() -> usize {
    pref_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| PROVIDERS.iter().position(|p| p.name == t.trim()))
        .unwrap_or(0)
}

fn save_selected(i: usize) {
    if let Some(p) = pref_path() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, PROVIDERS[i].name);
    }
}

// ---------- system ----------

fn powershell(script: &str) -> Result<String, String> {
    let out = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

const ACTIVE: &str = "Get-NetAdapter -Physical | Where-Object Status -eq 'Up'";

/// 현재 적용 중인 제공자의 인덱스. 우리가 관리하는 주소가 아니면 None.
fn dns_enabled() -> Option<usize> {
    let script = format!("({ACTIVE} | Get-DnsClientServerAddress | ForEach-Object ServerAddresses) -join ','");
    let out = powershell(&script).ok()?;
    let addrs: Vec<&str> = out.split(',').map(str::trim).collect();
    PROVIDERS.iter().position(|p| addrs.contains(&p.v4[0]))
}

fn set_dns(enable: bool, idx: usize) -> Result<(), String> {
    let apply = if enable {
        let p = &PROVIDERS[idx];
        let list: Vec<String> = p.v4.iter().chain(p.v6.iter()).map(|a| format!("'{a}'")).collect();
        format!("Set-DnsClientServerAddress -InterfaceIndex $_.ifIndex -ServerAddresses {}", list.join(","))
    } else {
        "Set-DnsClientServerAddress -InterfaceIndex $_.ifIndex -ResetServerAddresses".to_string()
    };
    let script = format!(
        "$ErrorActionPreference='Stop'; {ACTIVE} | ForEach-Object {{ {apply} }}; Clear-DnsClientCache"
    );
    powershell(&script).map(|_| ())
}

fn autostart_enabled() -> bool {
    Command::new("schtasks")
        .args(["/Query", "/TN", TASK_NAME])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn set_autostart(enable: bool) -> Result<(), String> {
    let script = if enable {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let exe = exe.to_string_lossy().replace('\'', "''");
        format!(
            "$ErrorActionPreference='Stop'; \
             $a = New-ScheduledTaskAction -Execute '{exe}' -Argument '--tray'; \
             $t = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME; \
             $p = New-ScheduledTaskPrincipal -UserId $env:USERNAME -RunLevel Highest -LogonType Interactive; \
             $s = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero); \
             Register-ScheduledTask -TaskName '{TASK_NAME}' -Action $a -Trigger $t -Principal $p -Settings $s -Force | Out-Null"
        )
    } else {
        format!("$ErrorActionPreference='Stop'; Unregister-ScheduledTask -TaskName '{TASK_NAME}' -Confirm:$false")
    };
    powershell(&script).map(|_| ())
}

// ---------- icons ----------

fn icon_rgba(color: Color32, size: u32) -> Vec<u8> {
    let s = size as f32;
    let (cx, cy) = (s / 2.0 - 0.5, s / 2.0 - 0.5);
    let corner = s * 0.14;
    let mut px = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (x as f32, y as f32);
            let (dx, dy) = ((fx - cx).abs() - (s / 2.0 - corner), (fy - cy).abs() - (s / 2.0 - corner));
            let outside = dx.max(0.0).hypot(dy.max(0.0)) > corner;
            let d = (fx - cx).hypot(fy - cy);
            let ring = d < s * 0.30 && d > s * 0.18;
            let rgba = if outside {
                [0, 0, 0, 0]
            } else if ring {
                [255, 255, 255, 255]
            } else {
                [color.r(), color.g(), color.b(), 255]
            };
            px.extend_from_slice(&rgba);
        }
    }
    px
}

fn tray_icon_for(on: bool) -> Icon {
    Icon::from_rgba(icon_rgba(if on { RED } else { BLACK }, 32), 32, 32).unwrap()
}

// ---------- app ----------

enum Msg {
    Dns(Result<Option<usize>, String>),
    Autostart(Result<bool, String>),
}

enum TrayCmd {
    Show,
    Toggle,
    Quit,
}

struct App {
    dns_on: bool,
    sel: usize,
    autostart: bool,
    busy: bool,
    error: Option<String>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    tray_rx: Receiver<TrayCmd>,
    tray: TrayIcon,
    toggle_item: MenuItem,
    quit: bool,
    phase: f32,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        setup_style(&cc.egui_ctx);

        let menu = Menu::new();
        let sel = load_selected();
        let show = MenuItem::new("열기", true, None);
        let toggle_item = MenuItem::new(format!("{} DNS 켜기", PROVIDERS[sel].name), true, None);
        let quit = MenuItem::new("종료", true, None);
        menu.append_items(&[&show, &toggle_item, &PredefinedMenuItem::separator(), &quit]).unwrap();
        let (show_id, toggle_id, quit_id) = (show.id().clone(), toggle_item.id().clone(), quit.id().clone());

        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_tooltip("AdBlocker-thinkgood")
            .with_icon(tray_icon_for(false))
            .build()
            .unwrap();

        let (tray_tx, tray_rx) = mpsc::channel();
        {
            let (ctx, tx) = (cc.egui_ctx.clone(), tray_tx.clone());
            TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
                if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                    let _ = tx.send(TrayCmd::Show);
                    ctx.request_repaint();
                }
            }));
        }
        {
            let ctx = cc.egui_ctx.clone();
            MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
                let cmd = if e.id == show_id {
                    TrayCmd::Show
                } else if e.id == toggle_id {
                    TrayCmd::Toggle
                } else if e.id == quit_id {
                    TrayCmd::Quit
                } else {
                    return;
                };
                let _ = tray_tx.send(cmd);
                ctx.request_repaint();
            }));
        }

        let (tx, rx) = mpsc::channel();
        let mut app = Self {
            dns_on: false,
            sel,
            autostart: autostart_enabled(),
            busy: false,
            error: None,
            tx,
            rx,
            tray_rx,
            tray,
            toggle_item,
            quit: false,
            phase: 0.0,
        };
        app.refresh(&cc.egui_ctx);
        app
    }

    fn refresh(&mut self, ctx: &egui::Context) {
        self.busy = true;
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Dns(Ok(dns_enabled())));
            ctx.request_repaint();
        });
    }

    fn toggle_dns(&mut self, ctx: &egui::Context) {
        self.apply_dns(ctx, !self.dns_on);
    }

    /// 이미 켜져 있어도 끄지 않고 바로 덮어쓴다.
    fn apply_dns(&mut self, ctx: &egui::Context, target: bool) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.error = None;
        let idx = self.sel;
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Dns(set_dns(target, idx).map(|_| target.then_some(idx))));
            ctx.request_repaint();
        });
    }

    fn select_provider(&mut self, ctx: &egui::Context, i: usize) {
        if self.busy || i == self.sel {
            return;
        }
        self.sel = i;
        save_selected(i);
        if self.dns_on {
            self.apply_dns(ctx, true);
        } else {
            self.toggle_item.set_text(format!("{} DNS 켜기", PROVIDERS[i].name));
        }
    }

    fn toggle_autostart(&mut self, ctx: &egui::Context) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.error = None;
        let target = !self.autostart;
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Autostart(set_autostart(target).map(|_| target)));
            ctx.request_repaint();
        });
    }

    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.rx.try_recv() {
            self.busy = false;
            match msg {
                Msg::Dns(Ok(active)) => {
                    let on = active.is_some();
                    self.dns_on = on;
                    if let Some(i) = active {
                        self.sel = i;
                        save_selected(i);
                    }
                    let name = PROVIDERS[self.sel].name;
                    let _ = self.tray.set_icon(Some(tray_icon_for(on)));
                    let _ = self.tray.set_tooltip(Some(format!("{name} DNS · {}", if on { "켜짐" } else { "꺼짐" })));
                    self.toggle_item.set_text(format!("{name} DNS {}", if on { "끄기" } else { "켜기" }));
                }
                Msg::Autostart(Ok(v)) => self.autostart = v,
                Msg::Dns(Err(e)) | Msg::Autostart(Err(e)) => self.error = Some(e),
            }
        }
        while let Ok(cmd) = self.tray_rx.try_recv() {
            match cmd {
                TrayCmd::Show => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                TrayCmd::Toggle => self.toggle_dns(ctx),
                TrayCmd::Quit => {
                    self.quit = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
    }
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [1.0, 1.0, 1.0, 1.0]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll(&ctx);

        if ctx.input(|i| i.viewport().close_requested()) && !self.quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }

        let full = ui.max_rect();
        ui.painter().rect_filled(full, 0.0, WHITE);

        egui::Frame::NONE.fill(WHITE).inner_margin(24.0).show(ui, |ui| {
            // header
            ui.label(RichText::new(format!("{} DNS", PROVIDERS[self.sel].name)).size(18.0).strong().color(BLACK));
            ui.add_space(20.0);

            // status + orb
            let (label, color) = if self.dns_on { ("보호 중", RED) } else { ("꺼짐", GRAY) };
            let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 84.0), egui::Sense::hover());
            let p = ui.painter();
            p.text(rect.left_top(), egui::Align2::LEFT_TOP, "상태", egui::FontId::proportional(12.0), GRAY);
            p.text(
                rect.left_top() + Vec2::new(0.0, 22.0),
                egui::Align2::LEFT_TOP,
                label,
                egui::FontId::proportional(30.0),
                color,
            );
            let k = ctx.animate_value_with_time(egui::Id::new("orb_k"), if self.dns_on { 1.0 } else { 0.0 }, 0.9);
            self.phase += ctx.input(|i| i.stable_dt) * k;
            let t = ctx.input(|i| i.time) as f32;
            draw_orb(ui.painter(), rect.right_center() - Vec2::new(46.0, 0.0), t, self.phase, k);
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
            ui.add_space(16.0);

            // main button
            let (text, fill) = if self.dns_on { ("끄기", BLACK) } else { ("켜기", RED) };
            let btn = egui::Button::new(
                RichText::new(if self.busy { "처리 중…" } else { text }).size(16.0).strong().color(WHITE),
            )
            .fill(fill)
            .corner_radius(CornerRadius::same(R))
            .min_size(Vec2::new(ui.available_width(), 46.0));
            if ui.add_enabled(!self.busy, btn).clicked() {
                self.toggle_dns(&ctx);
            }
            if let Some(e) = &self.error {
                ui.add_space(8.0);
                ui.label(RichText::new(e).size(11.0).color(RED));
            }

            ui.add_space(20.0);
            divider(ui);
            ui.add_space(12.0);

            // provider
            ui.label(RichText::new("DNS 제공자").size(12.0).color(GRAY));
            ui.add_space(4.0);
            for (i, p) in PROVIDERS.iter().enumerate() {
                if provider_row(ui, p, i == self.sel, !self.busy) {
                    self.select_provider(&ctx, i);
                }
                ui.add_space(4.0);
            }

            ui.add_space(8.0);
            divider(ui);
            ui.add_space(12.0);

            // options
            let mut auto = self.autostart;
            if switch_row(ui, "시작 프로그램으로 등록", &mut auto, !self.busy) {
                self.toggle_autostart(&ctx);
            }
            ui.add_space(6.0);
            let tray_btn = egui::Button::new(RichText::new("트레이로 내리기").color(BLACK))
                .fill(WHITE)
                .stroke(Stroke::new(1.0, BLACK))
                .corner_radius(CornerRadius::same(R))
                .min_size(Vec2::new(ui.available_width(), 34.0));
            if ui.add(tray_btn).clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }

            ui.add_space(12.0);
            divider(ui);
            ui.add_space(12.0);

            // servers
            ui.label(RichText::new("서버").size(12.0).color(GRAY));
            let cur = &PROVIDERS[self.sel];
            ui.label(RichText::new(cur.host).size(12.0).color(BLACK));
            for a in cur.v4.iter().chain(cur.v6.iter()) {
                ui.label(RichText::new(*a).monospace().size(12.0).color(BLACK));
            }
        });
    }
}

/// Rotate a point: roll (z), then yaw (y), then tilt (x).
fn rot3(v: [f32; 3], roll: f32, yaw: f32, tilt: f32) -> [f32; 3] {
    let [x, y, z] = v;
    let (x, y) = (x * roll.cos() - y * roll.sin(), x * roll.sin() + y * roll.cos());
    let (x, z) = (x * yaw.cos() + z * yaw.sin(), -x * yaw.sin() + z * yaw.cos());
    let (y, z) = (y * tilt.cos() - z * tilt.sin(), y * tilt.sin() + z * tilt.cos());
    [x, y, z]
}

fn draw_orb(p: &egui::Painter, base: egui::Pos2, t: f32, phase: f32, k: f32) {
    use std::f32::consts::{PI, TAU};
    let bob = (t * 1.5).sin();
    let c = base + Vec2::new(0.0, bob * 3.5); // always gently floating
    let r = 22.0;
    let col = Color32::from_rgb(0x55, 0x55, 0x5C).lerp_to_gamma(RED, k);
    let pulse = (phase * 2.4).sin() * 0.5 + 0.5;
    let at = |v: [f32; 3], rad: f32| egui::pos2(c.x + v[0] * rad, c.y + v[1] * rad);

    // ground shadow, shrinks as the orb floats up
    let sh = 1.0 - bob * 0.12;
    p.add(egui::Shape::Ellipse(egui::epaint::EllipseShape::filled(
        base + Vec2::new(0.0, 40.0),
        Vec2::new(r * 0.8 * sh, 3.0 * sh),
        Color32::from_black_alpha(22),
    )));

    // aura
    for i in 0..6 {
        let rr = r + 2.0 + i as f32 * 4.5 + pulse * 4.0 * k;
        let a = (0.04 + 0.15 * k) * (1.0 - i as f32 / 6.0);
        p.circle_filled(c, rr, col.gamma_multiply(a));
    }

    // two orbital rings (tilted); back halves behind the sphere, front halves in front
    let rings = [(0.35_f32, 1.25_f32, 1.0_f32), (-0.9, 1.0, -1.0)];
    let ring_pts = |roll: f32, tilt: f32, dir: f32| -> Vec<[f32; 3]> {
        let spin = phase * 0.6 * dir;
        (0..=72)
            .map(|n| {
                let a = n as f32 / 72.0 * TAU;
                rot3([a.cos(), 0.0, a.sin()], roll, spin, tilt)
            })
            .collect()
    };
    let ring_r = r * 1.55;
    let ring_alpha = 0.18 + 0.55 * k;
    let draw_ring = |front: bool| {
        for &(roll, tilt, dir) in &rings {
            let pts = ring_pts(roll, tilt, dir);
            for w in pts.windows(2) {
                let z = (w[0][2] + w[1][2]) * 0.5;
                if (z >= 0.0) != front {
                    continue;
                }
                let depth = 0.45 + 0.55 * z.abs().min(1.0) * if front { 1.0 } else { 0.4 };
                p.line_segment(
                    [at(w[0], ring_r), at(w[1], ring_r)],
                    Stroke::new(if front { 1.4 } else { 0.9 }, col.gamma_multiply(ring_alpha * depth)),
                );
            }
        }
    };
    draw_ring(false);

    // sphere body: layered gradient with an off-center light
    let dark = Color32::BLACK.lerp_to_gamma(col, 0.40);
    let bright = col.lerp_to_gamma(WHITE, 0.55);
    let n = 28;
    for i in 0..n {
        let f = i as f32 / (n - 1) as f32;
        let rad = r * (1.0 - f * 0.88);
        let ctr = c + Vec2::new(-0.30, -0.36) * r * f;
        p.circle_filled(ctr, rad, dark.lerp_to_gamma(bright, f.powf(1.6)));
    }

    // wireframe globe (front side only), rotating
    let yaw = phase * 0.9;
    let tilt = 0.45;
    let wire_a = 0.20 + 0.55 * k;
    let wire = col.lerp_to_gamma(WHITE, 0.6);
    let mut line = |pts: Vec<[f32; 3]>| {
        for w in pts.windows(2) {
            let (a, b) = (rot3(w[0], 0.0, yaw, tilt), rot3(w[1], 0.0, yaw, tilt));
            let z = (a[2] + b[2]) * 0.5;
            if z <= 0.0 {
                continue;
            }
            p.line_segment([at(a, r), at(b, r)], Stroke::new(0.8, wire.gamma_multiply(wire_a * z)));
        }
    };
    for i in 0..6 {
        let phi = i as f32 * PI / 6.0;
        line((0..=48)
            .map(|n| {
                let th = n as f32 / 48.0 * TAU;
                [th.sin() * phi.cos(), th.cos(), th.sin() * phi.sin()]
            })
            .collect());
    }
    for lat in [-PI / 4.0, 0.0, PI / 4.0] {
        line((0..=48)
            .map(|n| {
                let th = n as f32 / 48.0 * TAU;
                [lat.cos() * th.cos(), lat.sin(), lat.cos() * th.sin()]
            })
            .collect());
    }

    // glowing core + specular + fresnel rim
    p.circle_filled(c, 7.0 + pulse * 2.5 * k, WHITE.gamma_multiply(0.10 + 0.30 * k));
    p.circle_filled(c, 3.0, WHITE.gamma_multiply(0.15 + 0.55 * k));
    p.circle_filled(c + Vec2::new(-8.0, -10.0), 4.0, WHITE.gamma_multiply(0.45));
    p.circle_stroke(c, r, Stroke::new(3.0, col.gamma_multiply(0.18 + 0.15 * k)));
    p.circle_stroke(c, r - 0.5, Stroke::new(1.2, bright.gamma_multiply(0.45 + 0.4 * pulse * k)));

    draw_ring(true);

    // light nodes travelling along the rings
    for (i, &(roll, tilt, dir)) in rings.iter().enumerate() {
        let a = phase * 1.7 * dir + i as f32 * 2.1;
        let v = rot3(rot3([a.cos(), 0.0, a.sin()], roll, phase * 0.6 * dir, tilt), 0.0, 0.0, 0.0);
        if v[2] < -0.2 && k < 0.99 {
            continue;
        }
        let pos = at(v, ring_r);
        let s = 0.25 + 0.75 * k;
        p.circle_filled(pos, 5.0 * s, col.gamma_multiply(0.25 * s));
        p.circle_filled(pos, 2.2 * s.max(0.5), WHITE.gamma_multiply(0.9 * s.max(0.3)));
    }
}

/// 제공자 선택 행: 왼쪽에 이름/설명, 오른쪽에 라디오. 클릭되면 true.
fn provider_row(ui: &mut egui::Ui, p: &Provider, selected: bool, enabled: bool) -> bool {
    let size = Vec2::new(ui.available_width(), 44.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let pa = ui.painter();
    let line = if selected { RED } else { LINE };
    pa.rect_stroke(rect, CornerRadius::same(R), Stroke::new(1.0, line), egui::StrokeKind::Inside);
    pa.text(rect.left_top() + Vec2::new(12.0, 7.0), egui::Align2::LEFT_TOP, p.name, egui::FontId::proportional(14.0), BLACK);
    pa.text(rect.left_top() + Vec2::new(12.0, 26.0), egui::Align2::LEFT_TOP, p.note, egui::FontId::proportional(11.0), GRAY);
    let c = rect.right_center() - Vec2::new(20.0, 0.0);
    pa.circle_stroke(c, 8.0, Stroke::new(2.0, line));
    if selected {
        pa.circle_filled(c, 4.0, RED);
    }
    if resp.hovered() && enabled {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    enabled && resp.clicked()
}

fn divider(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, LINE);
}

/// Label on the left, flat switch on the right. Returns true when clicked.
fn switch_row(ui: &mut egui::Ui, text: &str, on: &mut bool, enabled: bool) -> bool {
    let size = Vec2::new(ui.available_width(), 30.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let p = ui.painter();
    p.text(rect.left_center(), egui::Align2::LEFT_CENTER, text, egui::FontId::proportional(14.0), BLACK);
    let track = egui::Rect::from_center_size(rect.right_center() - Vec2::new(20.0, 0.0), Vec2::new(40.0, 20.0));
    p.rect_filled(track, CornerRadius::same(R), if *on { RED } else { LINE });
    let knob_x = if *on { track.right() - 10.0 } else { track.left() + 10.0 };
    let knob = egui::Rect::from_center_size(egui::pos2(knob_x, track.center().y), Vec2::splat(14.0));
    p.rect_filled(knob, CornerRadius::same(R), WHITE);
    if resp.hovered() && enabled {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    enabled && resp.clicked()
}

fn setup_style(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "noto".into(),
        std::sync::Arc::new(FontData::from_static(include_bytes!("../NotoSansKR-Regular.ttf"))),
    );
    for fam in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts.families.get_mut(&fam).unwrap().push("noto".into());
    }
    fonts.families.get_mut(&FontFamily::Proportional).unwrap().insert(0, "noto".into());
    ctx.set_fonts(fonts);

    let mut v = egui::Visuals::light();
    v.panel_fill = WHITE;
    v.window_fill = WHITE;
    v.override_text_color = Some(BLACK);
    v.selection.bg_fill = RED;
    let r = CornerRadius::same(R);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = r;
    }
    ctx.set_visuals(v);
}

fn main() -> eframe::Result {
    let hidden = std::env::args().any(|a| a == "--tray");
    let icon = egui::IconData { rgba: icon_rgba(RED, 64), width: 64, height: 64 };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("AdBlocker-thinkgood")
            .with_inner_size([340.0, 700.0])
            .with_resizable(false)
            .with_maximize_button(false)
            .with_visible(!hidden)
            .with_icon(icon),
        ..Default::default()
    };
    eframe::run_native("AdBlocker-thinkgood", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}
