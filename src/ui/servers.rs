//! The server browser tab: TS servers from every source we know, their players, and joining one;
//! plus "Start a server", which hosts a game (with bots) on this machine.

use std::net::SocketAddr;
use std::process::Child;
use std::time::Duration;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::select::{Select, SelectState};
use gpui_kit::component::table::{Column, ColumnSort, DataTable, TableDelegate, TableEvent, TableState};
use gpui_kit::component::{ActiveTheme, Icon, IconName, IndexPath, Sizable, StyledExt, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use ts_launcher::install::{TsLibs, ts_libs};
use ts_launcher::server_host::{self, BOT_MODELS, BotSupport, GameMode, HostSettings, ServerKind};
use ts_launcher::servers::{self, Event, PlayerInfo, ServerInfo, Source};

use super::{AppState, section};

/// Seconds a dedicated server gets to load its map before the game joins it.
const JOIN_DELAY: Duration = Duration::from_secs(4);
const SKILLS: [(&str, Option<f32>); 5] =
    [("Random", None), ("Easy (0.25)", Some(0.25)), ("Normal (0.5)", Some(0.5)), ("Hard (0.75)", Some(0.75)), ("Expert (1.0)", Some(1.0))];

/// One server the browser knows about.
struct Row {
    addr: SocketAddr,
    /// The address as its source wrote it (a host name stays a host name).
    label: String,
    sources: Vec<Source>,
    /// The entry in the store's favorites that resolved to this server.
    favorite: Option<String>,
    info: Option<ServerInfo>,
    ping: Option<Duration>,
    error: Option<String>,
}

impl Row {
    fn new(addr: SocketAddr, label: String) -> Self {
        Self { addr, label, sources: Vec::new(), favorite: None, info: None, ping: None, error: None }
    }

    /// Favorites always show; anything else only once it answers as a TS server.
    fn listed(&self) -> bool {
        self.favorite.is_some() || self.info.as_ref().is_some_and(ServerInfo::is_ts)
    }
}

/// A row as the table shows it.
#[derive(Clone)]
struct RowView {
    addr: SocketAddr,
    favorite: bool,
    password: bool,
    name: String,
    map: String,
    players: u8,
    max_players: u8,
    bots: u8,
    ping: Option<u32>,
    address: String,
    sources: String,
    answered: bool,
}

const COLUMNS: [(&str, &str, f32); 8] = [
    ("fav", "", 30.),
    ("lock", "", 28.),
    ("name", "Name", 226.),
    ("map", "Map", 116.),
    ("players", "Players", 112.),
    ("ping", "Ping", 76.),
    ("address", "Address", 160.),
    ("source", "Source", 80.),
];

struct ServerTable {
    rows: Vec<RowView>,
    /// The column sorted by, if the user clicked one.
    sort: Option<(usize, ColumnSort)>,
    tab: WeakEntity<ServersTab>,
}

impl ServerTable {
    fn apply_sort(&mut self) {
        let by_players = |a: &RowView, b: &RowView| {
            b.answered.cmp(&a.answered).then(b.players.cmp(&a.players)).then(a.ping.unwrap_or(u32::MAX).cmp(&b.ping.unwrap_or(u32::MAX)))
        };
        match self.sort {
            Some((col, sort)) if sort != ColumnSort::Default => {
                self.rows.sort_by(|a, b| {
                    let o = match COLUMNS[col].0 {
                        "fav" => a.favorite.cmp(&b.favorite),
                        "lock" => a.password.cmp(&b.password),
                        "name" => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                        "map" => a.map.cmp(&b.map),
                        "players" => a.players.cmp(&b.players),
                        "ping" => a.ping.unwrap_or(u32::MAX).cmp(&b.ping.unwrap_or(u32::MAX)),
                        "address" => a.address.cmp(&b.address),
                        _ => a.sources.cmp(&b.sources),
                    };
                    if sort == ColumnSort::Descending { o.reverse() } else { o }
                });
            }
            _ => self.rows.sort_by(by_players),
        }
    }
}

impl TableDelegate for ServerTable {
    fn columns_count(&self, _: &App) -> usize {
        COLUMNS.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        let (key, name, width) = COLUMNS[col_ix];
        let col = Column::new(key, name).width(px(width));
        match key {
            "fav" | "lock" => col.resizable(false),
            "players" | "ping" => col.sortable().text_right(),
            _ => col.sortable(),
        }
    }

    fn perform_sort(&mut self, col_ix: usize, sort: ColumnSort, _: &mut Window, cx: &mut Context<TableState<Self>>) {
        self.sort = Some((col_ix, sort));
        self.apply_sort();
        let tab = self.tab.clone();
        // Keep the selection on the same server.
        cx.defer(move |cx| {
            let _ = tab.update(cx, |tab, cx| tab.rebuild(cx));
        });
    }

    fn render_td(&mut self, row_ix: usize, col_ix: usize, _: &mut Window, cx: &mut Context<TableState<Self>>) -> impl IntoElement {
        let Some(r) = self.rows.get(row_ix) else { return div().into_any_element() };
        let muted = cx.theme().muted_foreground;
        let text = |s: String| div().overflow_hidden().text_ellipsis().whitespace_nowrap().child(s).into_any_element();
        match COLUMNS[col_ix].0 {
            "fav" => {
                let (tab, addr, on) = (self.tab.clone(), r.addr, r.favorite);
                div()
                    .id(("star", row_ix))
                    .cursor_pointer()
                    .child(Icon::new(Lucide::Star).small().text_color(if on { cx.theme().warning } else { muted.opacity(0.4) }))
                    .on_click(move |_, _, cx| {
                        let _ = tab.update(cx, |tab, cx| tab.toggle_favorite(addr, cx));
                    })
                    .into_any_element()
            }
            "lock" if r.password => Icon::new(Lucide::Lock).small().text_color(muted).into_any_element(),
            "lock" => div().into_any_element(),
            "name" if !r.answered => div().text_color(muted).child(text(r.name.clone())).into_any_element(),
            "name" => text(r.name.clone()),
            "map" => text(r.map.clone()),
            "players" if !r.answered => div().into_any_element(),
            "players" => {
                let bots = if r.bots > 0 { format!(" ({} bots)", r.bots) } else { String::new() };
                text(format!("{}/{}{bots}", r.players, r.max_players))
            }
            "ping" => text(r.ping.map(|p| format!("{p} ms")).unwrap_or_else(|| "—".into())),
            "address" => div().text_color(muted).child(text(r.address.clone())).into_any_element(),
            _ => div().text_color(muted).child(text(r.sources.clone())).into_any_element(),
        }
    }

    fn render_empty(&mut self, _: &mut Window, cx: &mut Context<TableState<Self>>) -> impl IntoElement {
        let refreshing = self.tab.upgrade().is_some_and(|t| t.read(cx).refreshing);
        h_flex().size_full().justify_center().text_sm().text_color(cx.theme().muted_foreground).child(if refreshing {
            "Looking for servers…"
        } else {
            "No servers match. Try Refresh or clear the filter."
        })
    }
}

#[derive(Clone, Copy, PartialEq)]
enum View {
    Browser,
    Host,
}

/// The "Start a server" form's inputs.
struct HostForm {
    settings: HostSettings,
    kind: Entity<SelectState<Vec<SharedString>>>,
    map: Entity<SelectState<Vec<String>>>,
    mode: Entity<SelectState<Vec<SharedString>>>,
    skill: Entity<SelectState<Vec<SharedString>>>,
    team: Entity<SelectState<Vec<SharedString>>>,
    hostname: Entity<InputState>,
    port: Entity<InputState>,
    max_players: Entity<InputState>,
    password: Entity<InputState>,
    time_limit: Entity<InputState>,
    frag_limit: Entity<InputState>,
    round_time: Entity<InputState>,
    respawn_time: Entity<InputState>,
    bot_count: Entity<InputState>,
    bot_names: Entity<TextareaState>,
    /// Whether this install has a dedicated server binary.
    dedicated: bool,
    libs: Option<TsLibs>,
    maps: usize,
}

pub struct ServersTab {
    state: Entity<AppState>,
    /// The installation this tab shows.
    id: String,
    view: View,
    rows: Vec<Row>,
    refreshing: bool,
    /// Results from an older refresh are dropped.
    generation: u64,
    /// Problems with sources (Steam key rejected, a favorite that doesn't resolve, ...).
    notes: Vec<String>,
    table: Entity<TableState<ServerTable>>,
    filter: Entity<InputState>,
    hide_empty: bool,
    hide_full: bool,
    selected: Option<SocketAddr>,
    /// The selected server's players: `None` while loading.
    players: Option<Result<Vec<PlayerInfo>, String>>,
    add_favorite: Entity<InputState>,
    steam_key: Entity<InputState>,
    show_sources: bool,
    status: Option<Result<String, String>>,
    host: HostForm,
    /// A dedicated server started from here.
    dedicated: Option<Child>,
}

fn select_labels(
    labels: Vec<SharedString>,
    selected: usize,
    window: &mut Window,
    cx: &mut Context<ServersTab>,
) -> Entity<SelectState<Vec<SharedString>>> {
    cx.new(|cx| SelectState::new(labels, Some(IndexPath::new(selected)), window, cx))
}

impl ServersTab {
    pub fn new(state: Entity<AppState>, id: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let tab = cx.entity().downgrade();
        let table = cx.new(|cx| {
            TableState::new(ServerTable { rows: Vec::new(), sort: None, tab }, window, cx)
                .row_header(false)
                .col_selectable(false)
                .col_movable(false)
        });
        cx.subscribe(&table, |this, table, e: &TableEvent, cx| match e {
            TableEvent::SelectRow(ix) => {
                let addr = table.read(cx).delegate().rows.get(*ix).map(|r| r.addr);
                if addr.is_some() && addr != this.selected {
                    this.selected = addr;
                    this.load_players(cx);
                }
            }
            TableEvent::DoubleClickedRow(ix) => {
                if let Some(addr) = table.read(cx).delegate().rows.get(*ix).map(|r| r.addr) {
                    this.connect(addr, cx);
                }
            }
            _ => {}
        })
        .detach();

        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter by name, map or address"));
        cx.subscribe(&filter, |this, _, e: &InputEvent, cx| {
            if matches!(e, InputEvent::Change) {
                this.rebuild(cx);
            }
        })
        .detach();
        let add_favorite = cx.new(|cx| InputState::new(window, cx).placeholder("host:port"));
        cx.subscribe(&add_favorite, |this, _, e: &InputEvent, cx| {
            if matches!(e, InputEvent::PressEnter { .. }) {
                this.add_favorite(cx);
            }
        })
        .detach();
        let key = state.read(cx).store.steam_api_key.clone();
        let steam_key =
            cx.new(|cx| InputState::new(window, cx).placeholder("Steam Web API key (optional)").masked(true).default_value(key));
        cx.subscribe(&steam_key, |this, input, e: &InputEvent, cx| {
            if matches!(e, InputEvent::Change) {
                let key = input.read(cx).value().trim().to_string();
                let saved = this.state.update(cx, |s, _| {
                    s.store.steam_api_key = key;
                    s.save()
                });
                if let Err(e) = saved {
                    this.status = Some(Err(e));
                }
            }
        })
        .detach();

        let host = Self::host_form(&state, &id, window, cx);
        let mut this = Self {
            state,
            id,
            view: View::Browser,
            rows: Vec::new(),
            refreshing: false,
            generation: 0,
            notes: Vec::new(),
            table,
            filter,
            hide_empty: false,
            hide_full: false,
            selected: None,
            players: None,
            add_favorite,
            steam_key,
            show_sources: false,
            status: None,
            host,
            dedicated: None,
        };
        this.refresh(cx);
        this
    }

    fn host_form(state: &Entity<AppState>, id: &str, window: &mut Window, cx: &mut Context<Self>) -> HostForm {
        let inst = state.read(cx).instance(id).cloned();
        let ts = inst.as_ref().map(|i| i.ts_dir()).unwrap_or_default();
        let dedicated = inst.as_ref().is_some_and(|i| server_host::dedicated_binary(&i.install).is_some());
        let mut s = HostSettings::load();
        if !dedicated {
            s.kind = ServerKind::Listen;
        }
        let maps = server_host::maps(&ts);
        if !maps.contains(&s.map) && !maps.is_empty() {
            s.map = maps.iter().find(|m| *m == "ts_dojo").unwrap_or(&maps[0]).clone();
        }
        let input =
            |value: String, window: &mut Window, cx: &mut Context<Self>| cx.new(|cx| InputState::new(window, cx).default_value(value));
        let names = std::fs::read_to_string(ts.join("bots.txt")).unwrap_or_default().replace("\r\n", "\n");
        let kinds = vec!["Listen server: the game hosts it and you play".into(), "Dedicated server (no graphics)".into()];
        let map_ix = maps.iter().position(|m| *m == s.map);
        let mode_ix = GameMode::ALL.iter().position(|m| *m == s.mode).unwrap_or(0);
        let skill_ix = SKILLS.iter().position(|(_, v)| *v == s.bots.skill).unwrap_or(2);
        let team_ix = BOT_MODELS.iter().position(|m| *m == s.bots.team).map(|i| i + 1).unwrap_or(0);
        HostForm {
            kind: select_labels(kinds, usize::from(s.kind == ServerKind::Dedicated), window, cx),
            map: {
                let (n, maps) = (maps.len(), maps.clone());
                cx.new(|cx| SelectState::new(maps, map_ix.map(IndexPath::new), window, cx).searchable(n > 10))
            },
            mode: select_labels(GameMode::ALL.iter().map(|m| m.label().into()).collect(), mode_ix, window, cx),
            skill: select_labels(SKILLS.iter().map(|(l, _)| (*l).into()).collect(), skill_ix, window, cx),
            team: select_labels(
                std::iter::once("Random".into()).chain(BOT_MODELS.iter().map(|m| (*m).into())).collect(),
                team_ix,
                window,
                cx,
            ),
            hostname: input(s.hostname.clone(), window, cx),
            port: input(s.port.to_string(), window, cx),
            max_players: input(s.max_players.to_string(), window, cx),
            password: cx.new(|cx| InputState::new(window, cx).placeholder("none").default_value(s.password.clone())),
            time_limit: input(s.time_limit.to_string(), window, cx),
            frag_limit: input(s.frag_limit.to_string(), window, cx),
            round_time: input(s.round_time.to_string(), window, cx),
            respawn_time: input(s.respawn_time.to_string(), window, cx),
            bot_count: input(s.bots.count.to_string(), window, cx),
            bot_names: cx.new(|cx| TextareaState::new(window, cx).rows(6).default_value(names)),
            dedicated,
            libs: ts_libs(&ts),
            maps: maps.len(),
            settings: s,
        }
    }

    // ---- browser -------------------------------------------------------------------------------

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.refreshing = true;
        self.notes.clear();
        // Keep rows so the list doesn't flash empty; they are updated as answers come in.
        for r in &mut self.rows {
            r.sources.clear();
            r.favorite = None;
        }
        let (favorites, key) = {
            let store = &self.state.read(cx).store;
            (store.favorite_servers.clone(), store.steam_api_key.clone())
        };
        let (tx, rx) = async_channel::unbounded();
        servers::refresh(favorites, key, move |e| {
            let _ = tx.send_blocking(e);
        });
        cx.spawn(async move |this, cx| {
            while let Ok(first) = rx.recv().await {
                let mut batch = vec![first];
                while let Ok(e) = rx.try_recv() {
                    batch.push(e);
                }
                let alive = this.update(cx, |this, cx| {
                    if this.generation == generation {
                        batch.into_iter().for_each(|e| this.apply(e));
                        this.rebuild(cx);
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        self.load_players(cx);
        self.rebuild(cx);
    }

    fn row_mut(&mut self, addr: SocketAddr, label: &str) -> &mut Row {
        match self.rows.iter().position(|r| r.addr == addr) {
            Some(i) => &mut self.rows[i],
            None => {
                self.rows.push(Row::new(addr, label.to_owned()));
                self.rows.last_mut().unwrap()
            }
        }
    }

    fn apply(&mut self, e: Event) {
        match e {
            Event::Found { addr, label, source } => {
                let row = self.row_mut(addr, &label);
                // A name (from a list or a favorite) reads better than the LAN reply's IP.
                if source != Source::Lan && row.label.parse::<SocketAddr>().is_ok() {
                    row.label = label.clone();
                }
                if !row.sources.contains(&source) {
                    row.sources.push(source);
                }
                if source == Source::Favorite {
                    row.favorite = Some(label);
                }
            }
            Event::Info { addr, result } => {
                let row = self.row_mut(addr, &addr.to_string());
                match result {
                    Ok((info, ping)) => {
                        row.info = Some(info);
                        row.ping = Some(ping);
                        row.error = None;
                    }
                    Err(e) => {
                        row.info = None;
                        row.ping = None;
                        row.error = Some(e);
                    }
                }
            }
            Event::SourceFailed { source, error } => self.notes.push(format!("{}: {error}", source.label())),
            Event::Unresolved { label, error } => self.notes.push(format!("Favorite {label}: {error}")),
            Event::Done => {
                self.refreshing = false;
                // Rows no source mentions any more (a removed favorite) go.
                self.rows.retain(|r| !r.sources.is_empty());
            }
        }
    }

    /// Rebuilds the table rows from `self.rows`, the filter and the toggles.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let filter = self.filter.read(cx).value().trim().to_lowercase();
        let views: Vec<RowView> = self
            .rows
            .iter()
            .filter(|r| r.listed())
            .map(|r| {
                let info = r.info.clone().unwrap_or_default();
                RowView {
                    addr: r.addr,
                    favorite: r.favorite.is_some(),
                    password: info.password,
                    name: if r.info.is_some() {
                        info.name.clone()
                    } else {
                        r.error.clone().map(|e| format!("{} ({e})", r.label)).unwrap_or_else(|| format!("{} (asking…)", r.label))
                    },
                    map: info.map.clone(),
                    players: info.players,
                    max_players: info.max_players,
                    bots: info.bots,
                    ping: r.ping.map(|p| p.as_millis() as u32),
                    address: r.label.clone(),
                    sources: r.sources.iter().map(|s| s.label()).collect::<Vec<_>>().join(", "),
                    answered: r.info.is_some(),
                }
            })
            .filter(|v| {
                filter.is_empty()
                    || v.name.to_lowercase().contains(&filter)
                    || v.map.to_lowercase().contains(&filter)
                    || v.address.to_lowercase().contains(&filter)
            })
            .filter(|v| !(self.hide_empty && v.answered && v.players == 0))
            .filter(|v| !(self.hide_full && v.answered && v.players >= v.max_players))
            .collect();
        let selected = self.selected;
        self.table.update(cx, |t, cx| {
            let d = t.delegate_mut();
            d.rows = views;
            d.apply_sort();
            let ix = selected.and_then(|a| t.delegate().rows.iter().position(|r| r.addr == a));
            match ix {
                Some(ix) if t.selected_row() != Some(ix) => t.set_selected_row(ix, cx),
                None if t.selected_row().is_some() => t.clear_selection(cx),
                _ => {}
            }
            t.refresh(cx);
            cx.notify();
        });
        cx.notify();
    }

    fn load_players(&mut self, cx: &mut Context<Self>) {
        let Some(addr) = self.selected else {
            self.players = None;
            return;
        };
        self.players = None;
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { servers::query_players(addr).map_err(|e| format!("{e:#}")) }).await;
            let _ = this.update(cx, |this, cx| {
                if this.selected == Some(addr) {
                    this.players = Some(result);
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn toggle_favorite(&mut self, addr: SocketAddr, cx: &mut Context<Self>) {
        let Some(row) = self.rows.iter_mut().find(|r| r.addr == addr) else { return };
        let saved = self.state.update(cx, |s, _| {
            let favs = &mut s.store.favorite_servers;
            match &row.favorite {
                Some(f) => favs.retain(|x| x != f),
                None => favs.push(row.label.clone()),
            }
            s.save()
        });
        match row.favorite.take() {
            Some(_) => row.sources.retain(|s| *s != Source::Favorite),
            None => {
                row.favorite = Some(row.label.clone());
                row.sources.push(Source::Favorite);
            }
        }
        if let Err(e) = saved {
            self.status = Some(Err(e));
        }
        self.rows.retain(|r| !r.sources.is_empty());
        self.rebuild(cx);
    }

    fn add_favorite(&mut self, cx: &mut Context<Self>) {
        let addr = self.add_favorite.read(cx).value().trim().to_string();
        if addr.is_empty() {
            return;
        }
        let saved = self.state.update(cx, |s, _| {
            if !s.store.favorite_servers.contains(&addr) {
                s.store.favorite_servers.push(addr.clone());
            }
            s.save()
        });
        self.status = Some(saved.map(|_| format!("Added {addr} to your favorites")));
        // Clearing the input from inside its own event would re-enter it.
        let input = self.add_favorite.clone();
        cx.defer(move |cx| {
            if let Some(window) = cx.active_window() {
                let _ = window.update(cx, |_, window, cx| input.update(cx, |i, cx| i.set_value("", window, cx)));
            }
        });
        self.refresh(cx);
    }

    fn connect(&mut self, addr: SocketAddr, cx: &mut Context<Self>) {
        let target = addr.to_string();
        let args = vec!["+connect".to_owned(), target.clone()];
        let id = self.id.clone();
        if let Some(inst) = self.state.read(cx).instance(&id)
            && let Ok(cmd) = ts_launcher::launch::command(&inst.install, &inst.launch, &args)
        {
            log::info!("connect to {target}: {}", ts_launcher::launch::display(&cmd));
        }
        self.status = Some(self.state.update(cx, |s, cx| {
            cx.notify();
            if s.game_running() {
                return Err("The game is already running: join from its server browser, or quit it first.".into());
            }
            s.launch(&id, &args).map(|m| format!("{m}: joining {target}"))
        }));
        cx.notify();
    }

    fn render_browser(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let listed = self.rows.iter().filter(|r| r.listed()).count();
        let answered = self.rows.iter().filter(|r| r.listed() && r.info.is_some()).count();
        let summary = if self.refreshing {
            format!("Looking for servers… {answered} answered so far")
        } else {
            format!("{listed} server{}, {answered} answered", if listed == 1 { "" } else { "s" })
        };
        let selected = self.selected.and_then(|a| self.rows.iter().find(|r| r.addr == a));
        let toolbar = h_flex()
            .gap_3()
            .child(div().flex_1().child(Input::new(&self.filter).small().cleanable(true)))
            .child(Checkbox::new("hide-empty").label("Hide empty").checked(self.hide_empty).on_click(cx.listener(
                |this, c: &bool, _, cx| {
                    this.hide_empty = *c;
                    this.rebuild(cx);
                },
            )))
            .child(Checkbox::new("hide-full").label("Hide full").checked(self.hide_full).on_click(cx.listener(|this, c: &bool, _, cx| {
                this.hide_full = *c;
                this.rebuild(cx);
            })))
            .child(
                Button::new("refresh")
                    .outline()
                    .small()
                    .icon(Lucide::RefreshCw)
                    .label("Refresh")
                    .loading(self.refreshing)
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            )
            .child(Button::new("host").primary().small().icon(Lucide::Server).label("Start a server").on_click(cx.listener(
                |this, _, _, cx| {
                    this.view = View::Host;
                    this.status = None;
                    cx.notify();
                },
            )));

        let sources_row = h_flex().gap_2().text_sm().child(div().text_color(muted).child(summary)).child(div().flex_1()).child(
            Button::new("sources").ghost().xsmall().label(if self.show_sources { "Hide sources" } else { "Sources…" }).on_click(
                cx.listener(|this, _, _, cx| {
                    this.show_sources = !this.show_sources;
                    cx.notify();
                }),
            ),
        );

        let sources = v_flex()
            .gap_2()
            .p_3()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(border)
            .text_sm()
            .child(div().text_color(muted).child(
                "Servers come from the launcher's list (servers.txt in its repository), Xash3D FWGS's list, your LAN, \
                 your favorites and, with an API key, Steam's master server.",
            ))
            .child(
                h_flex()
                    .gap_2()
                    .child(div().w(px(130.)).child("Add a favorite"))
                    .child(div().flex_1().child(Input::new(&self.add_favorite).small()))
                    .child(
                        Button::new("add-fav")
                            .small()
                            .outline()
                            .icon(IconName::Plus)
                            .label("Add")
                            .on_click(cx.listener(|this, _, _, cx| this.add_favorite(cx))),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(div().w(px(130.)).child("Steam API key"))
                    .child(div().flex_1().child(Input::new(&self.steam_key).small().mask_toggle()))
                    .child(div().text_xs().text_color(muted).child("steamcommunity.com/dev/apikey")),
            );

        let details = match selected {
            None => {
                div().text_sm().text_color(muted).child("Select a server to see who is playing. Double-click to join.").into_any_element()
            }
            Some(r) => {
                let info = r.info.clone().unwrap_or_default();
                let addr = r.addr;
                let players: AnyElement = match &self.players {
                    None => div().text_sm().text_color(muted).child("Asking for players…").into_any_element(),
                    Some(Err(e)) => div().text_sm().text_color(muted).child(format!("No player list: {e}")).into_any_element(),
                    Some(Ok(p)) if p.is_empty() => div().text_sm().text_color(muted).child("Nobody is playing.").into_any_element(),
                    Some(Ok(p)) => v_flex()
                        .id("players")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scrollbar()
                        .children(p.iter().map(|p| {
                            let time = if p.duration < 0.0 {
                                "bot".to_owned()
                            } else {
                                let s = p.duration as u32;
                                format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
                            };
                            h_flex()
                                .gap_3()
                                .text_sm()
                                .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().child(p.name.clone()))
                                .child(div().w(px(50.)).text_right().child(p.score.to_string()))
                                .child(div().w(px(70.)).text_right().text_color(muted).child(time))
                        }))
                        .into_any_element(),
                };
                h_flex()
                    .gap_4()
                    .items_start()
                    .size_full()
                    .child(
                        v_flex()
                            .w(px(330.))
                            .flex_none()
                            .gap_1()
                            .text_sm()
                            .child(div().font_semibold().overflow_hidden().text_ellipsis().whitespace_nowrap().child(if r.info.is_some() {
                                info.name.clone()
                            } else {
                                r.label.clone()
                            }))
                            .child(div().text_color(muted).child(if r.label == r.addr.to_string() {
                                r.label.clone()
                            } else {
                                format!("{} ({})", r.label, r.addr)
                            }))
                            .when(r.info.is_some(), |d| {
                                d.child(div().child(format!(
                                    "{} · {} · {}",
                                    info.map,
                                    info.game,
                                    match (info.server_type, info.os) {
                                        ('l', _) => "listen server",
                                        (_, 'w') => "dedicated, Windows",
                                        (_, 'l') => "dedicated, Linux",
                                        _ => "dedicated",
                                    }
                                )))
                            })
                            .when_some(r.error.clone(), |d, e| d.child(div().text_color(muted).child(e)))
                            .child(
                                h_flex()
                                    .gap_2()
                                    .pt_2()
                                    .child(
                                        Button::new("connect")
                                            .primary()
                                            .small()
                                            .icon(IconName::Play)
                                            .label("Connect")
                                            .on_click(cx.listener(move |this, _, _, cx| this.connect(addr, cx))),
                                    )
                                    .child(
                                        Button::new("star")
                                            .outline()
                                            .small()
                                            .icon(Lucide::Star)
                                            .label(if r.favorite.is_some() { "Remove favorite" } else { "Add to favorites" })
                                            .on_click(cx.listener(move |this, _, _, cx| this.toggle_favorite(addr, cx))),
                                    ),
                            ),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .h_full()
                            .min_w_0()
                            .gap_1()
                            .child(
                                h_flex()
                                    .gap_3()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(div().flex_1().child("Player"))
                                    .child(div().w(px(50.)).text_right().child("Score"))
                                    .child(div().w(px(70.)).text_right().child("Time")),
                            )
                            .child(players),
                    )
                    .into_any_element()
            }
        };

        v_flex()
            .size_full()
            .p_4()
            .gap_2()
            .child(toolbar)
            .child(sources_row)
            .when(self.show_sources, |d| d.child(sources))
            .children(self.notes.iter().map(|n| div().text_xs().text_color(cx.theme().warning).child(n.clone())))
            .when_some(self.status.clone(), |d, s| {
                d.child(match s {
                    Ok(m) => Alert::success("servers-status", m).small().into_any_element(),
                    Err(e) => Alert::error("servers-status", e).small().into_any_element(),
                })
            })
            .child(div().flex_1().min_h(px(120.)).child(DataTable::new(&self.table).stripe(true)))
            .child(div().h(px(150.)).flex_none().p_3().rounded(cx.theme().radius).border_1().border_color(border).child(details))
            .into_any_element()
    }

    // ---- start a server ------------------------------------------------------------------------

    /// The form's values as settings, or what is wrong with them.
    fn read_form(&self, cx: &App) -> Result<HostSettings, String> {
        let f = &self.host;
        let text = |i: &Entity<InputState>| i.read(cx).value().trim().to_string();
        fn num<T: std::str::FromStr>(v: String, what: &str) -> Result<T, String> {
            v.parse().map_err(|_| format!("{what}: \"{v}\" is not a number"))
        }
        let ix = |s: &Entity<SelectState<Vec<SharedString>>>| s.read(cx).selected_index(cx).map(|i| i.row).unwrap_or(0);
        let mut s = f.settings.clone();
        s.kind = if f.dedicated && ix(&f.kind) == 1 { ServerKind::Dedicated } else { ServerKind::Listen };
        s.map = f.map.read(cx).selected_value().cloned().unwrap_or_default();
        s.mode = GameMode::ALL[ix(&f.mode).min(GameMode::ALL.len() - 1)];
        s.hostname = text(&f.hostname);
        s.password = text(&f.password);
        s.port = num(text(&f.port), "Port")?;
        s.max_players = num(text(&f.max_players), "Max players")?;
        s.time_limit = num(text(&f.time_limit), "Time limit")?;
        s.frag_limit = num(text(&f.frag_limit), "Frag limit")?;
        s.round_time = num(text(&f.round_time), "Round time")?;
        s.respawn_time = num(text(&f.respawn_time), "Respawn time")?;
        s.bots.count = num(text(&f.bot_count), "Bots")?;
        s.bots.skill = SKILLS[ix(&f.skill).min(SKILLS.len() - 1)].1;
        s.bots.team = match ix(&f.team) {
            0 => String::new(),
            i => BOT_MODELS.get(i - 1).map(|m| (*m).to_owned()).unwrap_or_default(),
        };
        Ok(s)
    }

    fn start_server(&mut self, cx: &mut Context<Self>) {
        self.status = Some(self.try_start_server(cx));
        cx.notify();
    }

    fn try_start_server(&mut self, cx: &mut Context<Self>) -> Result<String, String> {
        let s = self.read_form(cx)?;
        let inst = self.state.read(cx).instance(&self.id).cloned().ok_or("unknown installation")?;
        let ts = inst.ts_dir();
        s.validate(&ts).map_err(|e| format!("{e:#}"))?;
        self.host.settings = s.clone();
        if let Err(e) = s.save() {
            log::warn!("could not save the server settings: {e:#}");
        }
        let bots = server_host::bot_support(s.kind, self.host.libs);
        if s.bots.count > 0 && bots == BotSupport::Commands {
            let names = self.host.bot_names.read(cx).value().to_string();
            server_host::write_bot_names(&ts, &names).map_err(|e| format!("{e:#}"))?;
        }
        server_host::write_cfgs(&ts, &s, bots).map_err(|e| format!("{e:#}"))?;
        let bots_note = match (bots, s.bots.count) {
            (_, 0) => String::new(),
            (BotSupport::None, _) => " TS 3.0's bots only run on listen servers, so this server has none.".into(),
            (_, n) => format!(" {n} bots join a few seconds after the map loads."),
        };
        match s.kind {
            ServerKind::Listen => {
                let args = server_host::listen_args(&s);
                let id = self.id.clone();
                let msg = self.state.update(cx, |st, cx| {
                    cx.notify();
                    if st.game_running() {
                        return Err("The game is already running: quit it first.".to_owned());
                    }
                    st.launch(&id, &args)
                })?;
                log::info!("listen server: {}", args.join(" "));
                Ok(format!("{msg}: hosting {} on port {}.{bots_note}", s.map, s.port))
            }
            ServerKind::Dedicated => {
                if self.dedicated_running() {
                    return Err("A dedicated server started from here is still running: stop it first.".into());
                }
                let child = server_host::start_dedicated(&inst.install, &s).map_err(|e| format!("{e:#}"))?;
                let pid = child.id();
                self.dedicated = Some(child);
                if s.join {
                    let addr: SocketAddr = ([127, 0, 0, 1], s.port).into();
                    cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(JOIN_DELAY).await;
                        let _ = this.update(cx, |this, cx| {
                            if this.dedicated_running() {
                                let started = this.status.take();
                                this.connect(addr, cx);
                                if let (Some(Ok(a)), Some(Ok(b))) = (&started, &this.status) {
                                    this.status = Some(Ok(format!("{a} {b}.")));
                                }
                            }
                        });
                    })
                    .detach();
                }
                Ok(format!(
                    "Dedicated server started (pid {pid}) on port {}; its console log is ts/{}.{bots_note}",
                    s.port,
                    server_host::DEDICATED_LOG
                ))
            }
        }
    }

    fn dedicated_running(&mut self) -> bool {
        let alive = self.dedicated.as_mut().is_some_and(|c| matches!(c.try_wait(), Ok(None)));
        if !alive {
            self.dedicated = None;
        }
        alive
    }

    fn render_host(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let f = &self.host;
        let s = f.settings.clone();
        let dedicated_running = self.dedicated_running();
        let f = &self.host;
        let kind_dedicated = f.dedicated && f.kind.read(cx).selected_index(cx).is_some_and(|i| i.row == 1);
        let bots = server_host::bot_support(if kind_dedicated { ServerKind::Dedicated } else { ServerKind::Listen }, f.libs);
        let label = |t: &str| div().w(px(130.)).flex_none().text_sm().text_color(muted).child(t.to_owned());
        let field = |t: &str, el: AnyElement| h_flex().gap_3().child(label(t)).child(div().flex_1().min_w_0().child(el));
        let small_field = |t: &str, input: &Entity<InputState>, unit: &str| {
            h_flex()
                .gap_2()
                .child(label(t))
                .child(div().w(px(80.)).child(Input::new(input).small()))
                .child(div().text_sm().text_color(muted).child(unit.to_owned()))
        };
        let toggle = |id: &'static str, text: &'static str, on: bool, set: fn(&mut HostSettings, bool)| {
            Checkbox::new(id).label(text).checked(on).on_click(cx.listener(move |this, c: &bool, _, cx| {
                set(&mut this.host.settings, *c);
                cx.notify();
            }))
        };

        let server = section("Server", "", cx)
            .child(field(
                "Type",
                if f.dedicated {
                    Select::new(&f.kind).small().into_any_element()
                } else {
                    div()
                        .text_sm()
                        .child("Listen server: the game hosts it and you play. (This engine has no dedicated server build.)")
                        .into_any_element()
                },
            ))
            .when(kind_dedicated, |d| {
                d.child(field("", toggle("join", "Start the game and join it", s.join, |s, v| s.join = v).into_any_element()))
            })
            .child(field("Server name", Input::new(&f.hostname).small().into_any_element()))
            .child(field(
                "Map",
                Select::new(&f.map).small().placeholder(if f.maps == 0 { "no maps in ts/maps" } else { "choose a map" }).into_any_element(),
            ))
            .child(small_field("Max players", &f.max_players, ""))
            .child(small_field("Port", &f.port, "UDP; players connect to this port"))
            .child(field("Password", Input::new(&f.password).small().mask_toggle().into_any_element()))
            .child(field("", toggle("lan", "LAN only (sv_lan)", s.lan_only, |s, v| s.lan_only = v).into_any_element()));

        let game = section("Game", "", cx)
            .child(field("Mode", Select::new(&f.mode).small().into_any_element()))
            .child(small_field("Time limit", &f.time_limit, "minutes (0 = none)"))
            .child(small_field("Frag limit", &f.frag_limit, "(0 = none)"))
            .child(small_field("Round time", &f.round_time, "seconds (team and last-man-standing rounds)"))
            .child(small_field("Respawn time", &f.respawn_time, "seconds"))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_x_6()
                    .gap_y_2()
                    .child(toggle("ff", "Friendly fire", s.friendly_fire, |s, v| s.friendly_fire = v))
                    .child(toggle("powerups", "Powerups", s.powerups, |s, v| s.powerups = v))
                    .child(toggle("realbullet", "Simulated bullets", s.real_bullets, |s, v| s.real_bullets = v))
                    .child(toggle("usecash", "Buy system (money)", s.use_cash, |s, v| s.use_cash = v))
                    .child(toggle("slowmo", "Slow motion", s.slow_motion, |s, v| s.slow_motion = v)),
            );

        let bot_note = match bots {
            BotSupport::Commands => "Bots join a few seconds after each map starts. Names are picked from the list below (ts/bots.txt).",
            BotSupport::Quota => "The Rust port keeps this many bots on a dedicated server (ts_bot_quota); it names them itself.",
            BotSupport::None => "TS 3.0's bots only run on listen servers.",
        };
        let bots_section =
            section("Bots", bot_note, cx).child(small_field("Number of bots", &f.bot_count, "")).when(bots == BotSupport::Commands, |d| {
                d.child(field("Skill", Select::new(&f.skill).small().into_any_element()))
                    .child(field("Team / model", Select::new(&f.team).small().into_any_element()))
                    .child(
                        h_flex()
                            .gap_6()
                            .child(toggle("botlearn", "Learn the map", s.bots.learn, |s, v| s.bots.learn = v))
                            .child(toggle("bottalk", "Chat", s.bots.talk, |s, v| s.bots.talk = v))
                            .child(toggle("botarmor", "Armor", s.bots.armor, |s, v| s.bots.armor = v)),
                    )
                    .child(div().text_sm().child("Bot names, one per line (name//0.8 also sets that bot's skill)"))
                    .child(Textarea::new(&f.bot_names))
            });

        v_flex()
            .id("host-scroll")
            .size_full()
            .overflow_y_scrollbar()
            .p_4()
            .gap_3()
            .child(
                h_flex()
                    .gap_3()
                    .child(Button::new("back").ghost().small().icon(Lucide::ArrowLeft).label("Server list").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.view = View::Browser;
                            this.status = None;
                            cx.notify();
                        },
                    )))
                    .child(div().font_semibold().child("Start a server")),
            )
            .child(server)
            .child(game)
            .child(bots_section)
            .when_some(self.status.clone(), |d, s| {
                d.child(match s {
                    Ok(m) => Alert::success("host-status", m).into_any_element(),
                    Err(e) => Alert::error("host-status", e).into_any_element(),
                })
            })
            .child(
                h_flex()
                    .gap_3()
                    .child(
                        Button::new("start")
                            .primary()
                            .icon(IconName::Play)
                            .label("Start server")
                            .on_click(cx.listener(|this, _, _, cx| this.start_server(cx))),
                    )
                    .when(dedicated_running, |d| {
                        d.child(Button::new("stop").outline().icon(Lucide::CircleStop).label("Stop dedicated server").on_click(
                            cx.listener(|this, _, _, cx| {
                                if let Some(mut c) = this.dedicated.take() {
                                    let _ = c.kill();
                                    let _ = c.wait();
                                    this.status = Some(Ok("Dedicated server stopped".into()));
                                }
                                cx.notify();
                            }),
                        ))
                    }),
            )
            .into_any_element()
    }
}

impl Drop for ServersTab {
    fn drop(&mut self) {
        // A dedicated server outlives the tab only if the launcher quits; stop it with the tab.
        if let Some(mut c) = self.dedicated.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Render for ServersTab {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.view {
            View::Browser => self.render_browser(cx),
            View::Host => self.render_host(cx),
        }
    }
}
