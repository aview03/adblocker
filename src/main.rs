#![windows_subsystem = "windows"]

mod vpn;

use std::os::windows::process::CommandExt;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

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
    VpnList(Result<vpn::ServerList, String>),
    /// (세션 id, 국가, 결과)
    VpnResult(u64, String, Result<vpn::Connected, String>),
    VpnEnded(u64),
    /// 접속 진행 상황(탐색/시도별 결과)
    VpnEvent(u64, vpn::Event),
}

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Dns,
    Vpn,
}

/// DNS <-> VPN 전환 확인 대상.
#[derive(Clone, Copy)]
enum Switch {
    ToVpn,
    ToDns,
}

enum VpnList {
    NotLoaded,
    Loading,
    Ready(vpn::ServerList),
    Failed(String),
}

enum VpnState {
    Off,
    Connecting(String),
    On { country: String, ip: String },
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
    tab: Tab,
    vpn_list: VpnList,
    vpn: VpnState,
    vpn_id: u64,
    vpn_handle: vpn::Handle,
    vpn_cancel: Arc<AtomicBool>,
    /// 경고 모달에서 확인을 기다리는 국가(인덱스).
    vpn_confirm: Option<usize>,
    /// 목록에서 고른 국가(국가 코드). 켜기를 누르기 전에는 아무것도 적용되지 않는다.
    vpn_sel: Option<String>,
    /// 지금 접속(시도) 중인 국가 코드.
    vpn_conn: Option<String>,
    /// 상태 영역에 작게 보여줄 접속 진행 상황 (가장 최근 한 줄만)
    vpn_note: String,
    switch_confirm: Option<Switch>,
    openvpn: Option<std::path::PathBuf>,
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
            tab: Tab::Dns,
            vpn_list: VpnList::NotLoaded,
            vpn: VpnState::Off,
            vpn_id: 0,
            vpn_handle: Arc::new(Mutex::new(None)),
            vpn_cancel: Arc::new(AtomicBool::new(false)),
            vpn_confirm: None,
            vpn_sel: None,
            vpn_conn: None,
            vpn_note: String::new(),
            switch_confirm: None,
            openvpn: vpn::find_openvpn(),
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

    /// 선택한 국가가 현재 목록의 몇 번째인지.
    fn vpn_sel_idx(&self) -> Option<usize> {
        match (&self.vpn_list, &self.vpn_sel) {
            (VpnList::Ready(l), Some(s)) => l.countries.iter().position(|c| &c.short == s),
            _ => None,
        }
    }

    /// DNS 켜기 요청. VPN 이 쓰이는 중이면 바로 켜지 않고 전환을 먼저 묻는다.
    fn request_dns_on(&mut self, ctx: &egui::Context) {
        if !matches!(self.vpn, VpnState::Off) {
            self.switch_confirm = Some(Switch::ToDns);
        } else {
            self.apply_dns(ctx, true);
        }
    }

    fn tray_toggle(&mut self, ctx: &egui::Context) {
        if !self.dns_on && !matches!(self.vpn, VpnState::Off) {
            // 트레이에는 모달을 띄울 수 없으므로 창을 열어 확인을 받는다
            self.tab = Tab::Dns;
            self.switch_confirm = Some(Switch::ToDns);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        } else {
            self.toggle_dns(ctx);
        }
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

    fn vpn_active(&self) -> bool {
        matches!(self.vpn, VpnState::On { .. })
    }

    /// VPN 탭을 처음 열 때 한 번만 목록을 준비한다(캐시가 3일 이내면 네트워크 요청 없음).
    fn ensure_vpn_list(&mut self, ctx: &egui::Context) {
        if !matches!(self.vpn_list, VpnList::NotLoaded) {
            return;
        }
        self.vpn_list = VpnList::Loading;
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Msg::VpnList(vpn::load_list()));
            ctx.request_repaint();
        });
    }

    /// 사용자가 경고 모달에서 "접속"을 눌렀을 때만 호출된다.
    fn connect_vpn(&mut self, ctx: &egui::Context, country_idx: usize) {
        let Some(exe) = self.openvpn.clone() else {
            self.error = Some("OpenVPN 이 설치되어 있지 않습니다".into());
            return;
        };
        let VpnList::Ready(list) = &self.vpn_list else { return };
        let Some(country) = list.countries.get(country_idx) else { return };
        let label = country.label().to_string();
        self.vpn_conn = Some(country.short.clone());
        // 위에서 빌린 목록을 스레드로 넘기기 위해 필요한 부분만 복제
        let short = country.short.clone();

        // 이미 연결돼 있으면 끊고 새로 연결
        self.vpn_cancel.store(true, Ordering::Relaxed);
        vpn::disconnect(&self.vpn_handle);
        self.vpn_id += 1;
        let id = self.vpn_id;
        let cancel = Arc::new(AtomicBool::new(false));
        self.vpn_cancel = cancel.clone();
        self.vpn = VpnState::Connecting(label.clone());
        self.vpn_note.clear();
        self.error = None;

        let (handle, tx, ctx) = (self.vpn_handle.clone(), self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let (tx2, ctx2) = (tx.clone(), ctx.clone());
            let (tx3, ctx3) = (tx.clone(), ctx.clone());
            let on_event = move |e: vpn::Event| {
                let _ = tx3.send(Msg::VpnEvent(id, e));
                ctx3.request_repaint();
            };
            let res = vpn::connect(&exe, &short, &handle, &cancel, &on_event, move || {
                let _ = tx2.send(Msg::VpnEnded(id));
                ctx2.request_repaint();
            });
            let _ = tx.send(Msg::VpnResult(id, label, res));
            ctx.request_repaint();
        });
    }

    fn disconnect_vpn(&mut self) {
        self.vpn_cancel.store(true, Ordering::Relaxed);
        self.vpn_id += 1; // 이전 세션의 늦은 메시지 무시
        vpn::disconnect(&self.vpn_handle);
        self.vpn = VpnState::Off;
        self.vpn_conn = None;
        self.vpn_note.clear();
        self.update_tray();
    }

    fn update_tray(&self) {
        let name = PROVIDERS[self.sel].name;
        let on = self.dns_on || self.vpn_active();
        let _ = self.tray.set_icon(Some(tray_icon_for(on)));
        let tip = match &self.vpn {
            VpnState::On { country, .. } => format!("VPN 사용중 · {country}"),
            _ => format!("{name} DNS · {}", if self.dns_on { "켜짐" } else { "꺼짐" }),
        };
        let _ = self.tray.set_tooltip(Some(tip));
        self.toggle_item.set_text(format!("{name} DNS {}", if self.dns_on { "끄기" } else { "켜기" }));
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
            if matches!(msg, Msg::Dns(_) | Msg::Autostart(_)) {
                self.busy = false;
            }
            match msg {
                Msg::Dns(Ok(active)) => {
                    let on = active.is_some();
                    self.dns_on = on;
                    if let Some(i) = active {
                        self.sel = i;
                        save_selected(i);
                    }
                    self.update_tray();
                }
                Msg::Autostart(Ok(v)) => self.autostart = v,
                Msg::Dns(Err(e)) | Msg::Autostart(Err(e)) => self.error = Some(e),
                Msg::VpnList(r) => {
                    self.vpn_list = match r {
                        Ok(l) => VpnList::Ready(l),
                        Err(e) => VpnList::Failed(e),
                    }
                }
                Msg::VpnEvent(id, e) if id == self.vpn_id => match e {
                    vpn::Event::Probe { alive, total, secs } => {
                        self.vpn_note = format!("탐색 {secs:.1}초 · 응답 {alive}/{total}");
                    }
                    vpn::Event::Attempt(a) => {
                        self.vpn_note = format!("{}차 {} {:.1}초", a.n, a.reason, a.secs);
                    }
                    vpn::Event::ListRefreshed => {
                        self.vpn_note = "목록 갱신".to_string();
                        // 새 목록을 화면에도 반영 (이미 새로 받았으므로 네트워크 요청 없음)
                        self.vpn_list = VpnList::NotLoaded;
                        self.ensure_vpn_list(ctx);
                    }
                },
                Msg::VpnResult(id, country, r) if id == self.vpn_id => {
                    match r {
                        Ok(c) => {
                            self.vpn_note.push_str(&format!(" · 총 {:.1}초", c.total_secs));
                            self.vpn = VpnState::On { country, ip: c.ip }
                        }
                        Err(e) => {
                            self.vpn = VpnState::Off;
                            self.vpn_conn = None;
                            if e != "취소됨" {
                                self.error = Some(e);
                            }
                        }
                    }
                    self.update_tray();
                }
                Msg::VpnEnded(id) if id == self.vpn_id => {
                    self.vpn = VpnState::Off;
                    self.vpn_conn = None;
                    self.update_tray();
                }
                Msg::VpnResult(..) | Msg::VpnEnded(_) | Msg::VpnEvent(..) => {}
            }
        }
        while let Ok(cmd) = self.tray_rx.try_recv() {
            match cmd {
                TrayCmd::Show => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                TrayCmd::Toggle => self.tray_toggle(ctx),
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
            let title = match self.tab {
                Tab::Dns => format!("{} DNS", PROVIDERS[self.sel].name),
                Tab::Vpn => "VPN".to_string(),
            };
            ui.label(RichText::new(title).size(18.0).strong().color(BLACK));
            ui.add_space(20.0);

            // status + orb
            let vpn_on = self.vpn_active();
            let (label, color) = match &self.vpn {
                VpnState::On { .. } => ("VPN 사용중", RED),
                VpnState::Connecting(_) => ("연결 중…", GRAY),
                VpnState::Off if self.dns_on => ("보호 중", RED),
                VpnState::Off => ("꺼짐", GRAY),
            };
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
            let k = ctx.animate_value_with_time(egui::Id::new("orb_k"), if self.dns_on || vpn_on { 1.0 } else { 0.0 }, 0.9);
            self.phase += ctx.input(|i| i.stable_dt) * k;
            let t = ctx.input(|i| i.time) as f32;
            draw_orb(ui.painter(), rect.right_center() - Vec2::new(46.0, 0.0), t, self.phase, k);
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
            if self.tab == Tab::Vpn && !self.vpn_note.is_empty() {
                let mut line = self.vpn_note.clone();
                if matches!(self.vpn, VpnState::Connecting(_)) {
                    line.push_str(" · 다음 서버 시도 중…");
                }
                ui.add(egui::Label::new(RichText::new(line).size(10.5).color(GRAY)).truncate());
            }
            ui.add_space(16.0);

            // tabs
            ui.horizontal(|ui| {
                let w = (ui.available_width() - 6.0) / 2.0;
                for (t, name) in [(Tab::Dns, "DNS"), (Tab::Vpn, "VPN")] {
                    let on = self.tab == t;
                    let b = egui::Button::new(RichText::new(name).strong().color(if on { WHITE } else { BLACK }))
                        .fill(if on { BLACK } else { WHITE })
                        .stroke(Stroke::new(1.0, BLACK))
                        .corner_radius(CornerRadius::same(R))
                        .min_size(Vec2::new(w, 30.0));
                    if ui.add(b).clicked() && !on {
                        self.tab = t;
                        self.error = None;
                        if t == Tab::Vpn {
                            self.ensure_vpn_list(&ctx);
                        }
                    }
                }
            });
            ui.add_space(16.0);

            if self.tab == Tab::Vpn {
                self.vpn_tab(ui, &ctx);
                return;
            }

            // main button
            let (text, fill) = if self.dns_on { ("끄기", BLACK) } else { ("켜기", RED) };
            let btn = egui::Button::new(
                RichText::new(if self.busy { "처리 중…" } else { text }).size(16.0).strong().color(WHITE),
            )
            .fill(fill)
            .corner_radius(CornerRadius::same(R))
            .min_size(Vec2::new(ui.available_width(), 46.0));
            if ui.add_enabled(!self.busy, btn).clicked() {
                if self.dns_on {
                    self.toggle_dns(&ctx);
                } else {
                    self.request_dns_on(&ctx);
                }
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

        self.switch_modal(&ctx);
        self.vpn_modal(&ctx);
    }
}

impl App {
    fn vpn_tab(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let sel_idx = self.vpn_sel_idx();
        let locked = matches!(self.vpn, VpnState::Connecting(_)); // 연결 시도 중에만 선택 잠금
        // 접속 중인데 다른 국가를 골랐는가 → 버튼이 "전환"이 된다
        let switching = matches!(self.vpn, VpnState::On { .. }) && sel_idx.is_some() && self.vpn_sel != self.vpn_conn;

        // 메인 버튼 (DNS 탭과 같은 자리·같은 모양)
        let switch_text = match (&self.vpn_list, &self.vpn_sel) {
            (VpnList::Ready(l), Some(code)) => {
                let name = l.countries.iter().find(|c| &c.short == code).map(|c| c.label()).unwrap_or("");
                format!("전환 · {name}")
            }
            _ => "전환".to_string(),
        };
        let (text, fill, fg) = match &self.vpn {
            VpnState::On { .. } if switching => (switch_text.as_str(), RED, WHITE),
            VpnState::On { .. } => ("끄기", BLACK, WHITE),
            VpnState::Connecting(_) => ("취소", WHITE, BLACK),
            VpnState::Off => ("켜기", RED, WHITE),
        };
        let enabled = match self.vpn {
            VpnState::Off => sel_idx.is_some() && self.openvpn.is_some() && !self.busy,
            _ => true,
        };
        let mut btn = egui::Button::new(RichText::new(text).size(16.0).strong().color(fg))
            .fill(fill)
            .corner_radius(CornerRadius::same(R))
            .min_size(Vec2::new(ui.available_width(), 46.0));
        if matches!(self.vpn, VpnState::Connecting(_)) {
            btn = btn.stroke(Stroke::new(1.0, BLACK));
        }
        if ui.add_enabled(enabled, btn).clicked() {
            match (&self.vpn, sel_idx) {
                (VpnState::Off, Some(i)) => {
                    // 켜기: DNS 가 켜져 있으면 전환 확인 → 그 다음 접속 경고. 아니면 바로 접속 경고.
                    if self.dns_on {
                        self.switch_confirm = Some(Switch::ToVpn);
                    } else {
                        self.vpn_confirm = Some(i);
                    }
                }
                (VpnState::Off, None) => {}
                // 다른 국가로 전환: 접속 경고 → "접속" 시 기존 연결을 끊고 새로 접속
                (VpnState::On { .. }, Some(i)) if switching => self.vpn_confirm = Some(i),
                _ => self.disconnect_vpn(),
            }
        }

        // 상태 보조 문구
        ui.add_space(8.0);
        match &self.vpn {
            VpnState::On { country, ip } => {
                ui.label(RichText::new(format!("{country} · {ip}")).size(12.0).color(BLACK));
                if switching {
                    ui.label(
                        RichText::new("전환하면 현재 연결을 끊고 새 국가에 접속합니다.").size(11.0).color(GRAY),
                    );
                }
            }
            VpnState::Connecting(country) => {
                ui.label(RichText::new(format!("{country} 서버에 접속하는 중…")).size(12.0).color(GRAY));
            }
            VpnState::Off if sel_idx.is_none() => {
                ui.label(RichText::new("국가를 선택하면 켜기 버튼이 활성화됩니다.").size(12.0).color(GRAY));
            }
            VpnState::Off => {}
        }
        if let Some(e) = &self.error {
            ui.add_space(4.0);
            ui.label(RichText::new(e).size(11.0).color(RED));
        }
        if self.openvpn.is_none() {
            ui.add_space(4.0);
            ui.label(
                RichText::new("OpenVPN 이 필요합니다. openvpn.net 에서 설치한 뒤 앱을 다시 실행하세요.")
                    .size(11.0)
                    .color(RED),
            );
        }

        ui.add_space(16.0);
        divider(ui);
        ui.add_space(12.0);
        ui.label(RichText::new("국가").size(12.0).color(GRAY));
        ui.add_space(4.0);

        let mut picked: Option<String> = None;
        let mut retry = false;
        match &self.vpn_list {
            VpnList::NotLoaded | VpnList::Loading => {
                ui.label(RichText::new("목록 불러오는 중…").size(12.0).color(GRAY));
            }
            VpnList::Failed(e) => {
                ui.label(RichText::new(e).size(11.0).color(RED));
                ui.add_space(6.0);
                retry = ui.button("다시 시도").clicked();
            }
            VpnList::Ready(list) => {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    for c in list.countries.iter() {
                        let selected = self.vpn_sel.as_deref() == Some(c.short.as_str());
                        let connected = matches!(self.vpn, VpnState::On { .. })
                            && self.vpn_conn.as_deref() == Some(c.short.as_str());
                        if country_row(ui, c, selected, connected, !locked) {
                            picked = Some(c.short.clone());
                        }
                        ui.add_space(4.0);
                    }
                    ui.add_space(6.0);
                    let note = if list.stale {
                        format!("목록 갱신 실패 — {}일 전 목록 사용 중", list.age_days)
                    } else {
                        format!("목록 {}일 전 기준 · 3일마다 갱신", list.age_days)
                    };
                    ui.label(RichText::new(note).size(11.0).color(GRAY));
                });
            }
        }
        if retry {
            self.vpn_list = VpnList::NotLoaded;
            self.ensure_vpn_list(ctx);
        }
        if let Some(code) = picked {
            // 선택만 한다. 접속은 켜기 버튼 → (전환 확인) → 접속 경고의 "접속"에서만 일어난다.
            self.vpn_sel = Some(code);
            self.error = None;
        }
    }

    /// DNS <-> VPN 전환 확인.
    fn switch_modal(&mut self, ctx: &egui::Context) {
        let Some(sw) = self.switch_confirm else { return };
        let (title, body) = match sw {
            Switch::ToVpn => (
                "VPN 으로 전환",
                "DNS 보호가 켜져 있습니다. VPN 으로 전환할까요? (VPN 에 접속하면 DNS 보호는 꺼집니다)",
            ),
            Switch::ToDns => ("DNS 보호로 전환", "VPN 을 사용 중입니다. VPN 연결을 끊고 DNS 보호로 전환할까요?"),
        };
        let (mut yes, mut no) = (false, false);
        let modal = egui::Modal::new(egui::Id::new("switch_confirm")).show(ctx, |ui| {
            ui.set_width(270.0);
            ui.label(RichText::new(title).size(16.0).strong().color(BLACK));
            ui.add_space(8.0);
            ui.label(RichText::new(body).size(13.0).color(BLACK));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let w = (ui.available_width() - 6.0) / 2.0;
                let cancel = egui::Button::new(RichText::new("취소").color(BLACK))
                    .fill(WHITE)
                    .stroke(Stroke::new(1.0, BLACK))
                    .corner_radius(CornerRadius::same(R))
                    .min_size(Vec2::new(w, 36.0));
                let ok = egui::Button::new(RichText::new("전환").strong().color(WHITE))
                    .fill(BLACK)
                    .corner_radius(CornerRadius::same(R))
                    .min_size(Vec2::new(w, 36.0));
                no = ui.add(cancel).clicked();
                yes = ui.add(ok).clicked();
            });
        });
        if yes {
            self.switch_confirm = None;
            match sw {
                // 전환 확인을 닫은 뒤, VPN 접속 경고를 한 번 더 띄운다. (DNS 는 거기서 "접속"을 누를 때 끈다)
                Switch::ToVpn => self.vpn_confirm = self.vpn_sel_idx(),
                Switch::ToDns => {
                    self.disconnect_vpn();
                    self.apply_dns(ctx, true);
                }
            }
        } else if no || modal.should_close() {
            self.switch_confirm = None;
        }
    }

    fn vpn_modal(&mut self, ctx: &egui::Context) {
        let Some(i) = self.vpn_confirm else { return };
        let name = match &self.vpn_list {
            VpnList::Ready(l) => l.countries.get(i).map(|c| c.label().to_string()),
            _ => None,
        };
        let Some(name) = name else {
            self.vpn_confirm = None;
            return;
        };
        let (mut connect, mut cancel) = (false, false);
        let modal = egui::Modal::new(egui::Id::new("vpn_confirm")).show(ctx, |ui| {
            ui.set_width(270.0);
            ui.label(RichText::new("VPN 접속 경고").size(16.0).strong().color(RED));
            ui.add_space(8.0);
            ui.label(RichText::new(format!("{name} 의 공개 VPN 서버에 접속합니다.")).size(13.0).color(BLACK));
            ui.add_space(8.0);
            for t in [
                "VPN Gate 서버는 일반 자원봉사자가 운영합니다. 운영자가 내 통신 내용을 볼 수 있습니다.",
                "로그인·결제·금융 등 민감한 작업은 하지 마세요.",
                "접속하면 내 IP 가 바뀌고, 속도가 느리거나 끊길 수 있습니다.",
                "해당 국가의 법과 VPN Gate 이용 약관을 지켜야 합니다.",
            ] {
                ui.label(RichText::new(format!("• {t}")).size(12.0).color(BLACK));
                ui.add_space(3.0);
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let w = (ui.available_width() - 6.0) / 2.0;
                let no = egui::Button::new(RichText::new("취소").color(BLACK))
                    .fill(WHITE)
                    .stroke(Stroke::new(1.0, BLACK))
                    .corner_radius(CornerRadius::same(R))
                    .min_size(Vec2::new(w, 36.0));
                let yes = egui::Button::new(RichText::new("접속").strong().color(WHITE))
                    .fill(RED)
                    .corner_radius(CornerRadius::same(R))
                    .min_size(Vec2::new(w, 36.0));
                cancel = ui.add(no).clicked();
                connect = ui.add(yes).clicked();
            });
        });
        if connect {
            self.vpn_confirm = None;
            if self.dns_on {
                self.apply_dns(ctx, false); // DNS 보호 → VPN 전환
            }
            self.connect_vpn(ctx, i);
        } else if cancel || modal.should_close() {
            self.vpn_confirm = None;
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.vpn_cancel.store(true, Ordering::Relaxed);
        vpn::disconnect(&self.vpn_handle);
    }
}

/// 국가 행: 이름 + (서버 수 · 최고 속도 · 지연). 클릭되면 true.
fn country_row(ui: &mut egui::Ui, c: &vpn::Country, selected: bool, connected: bool, enabled: bool) -> bool {
    let size = Vec2::new(ui.available_width(), 44.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let pa = ui.painter();
    let hot = resp.hovered() && enabled;
    let line = if selected { RED } else if hot { BLACK } else { LINE };
    pa.rect_stroke(rect, CornerRadius::same(R), Stroke::new(1.0, line), egui::StrokeKind::Inside);
    let col = if enabled || selected { BLACK } else { GRAY };
    pa.text(rect.left_top() + Vec2::new(12.0, 7.0), egui::Align2::LEFT_TOP, c.label(), egui::FontId::proportional(14.0), col);
    let speed = c.best_speed();
    let detail = if speed > 0 {
        let ping = c.best_ping().map(|p| format!(" · {p}ms")).unwrap_or_default();
        format!("서버 {}개 · 최고 {:.0}Mbps{ping}", c.servers.len(), speed as f64 / 1e6)
    } else {
        format!("서버 {}개 · 속도 미확인", c.servers.len())
    };
    pa.text(rect.left_top() + Vec2::new(12.0, 26.0), egui::Align2::LEFT_TOP, detail, egui::FontId::proportional(11.0), GRAY);
    if connected {
        pa.text(
            rect.right_center() - Vec2::new(40.0, 0.0),
            egui::Align2::RIGHT_CENTER,
            "연결됨",
            egui::FontId::proportional(11.0),
            RED,
        );
    }
    let rc = rect.right_center() - Vec2::new(20.0, 0.0);
    pa.circle_stroke(rc, 8.0, Stroke::new(2.0, line));
    if selected {
        pa.circle_filled(rc, 4.0, RED);
    }
    if hot {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    enabled && resp.clicked()
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
