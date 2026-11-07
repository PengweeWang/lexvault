use ely_core::{
    buttons::Button,
    forms::{InputEvent, SearchInput, TextInput},
    layout::{Card, CardHeader, Page, Panel, ScrollArea},
    lists::{InfiniteList, ListItem},
    primitives::FocusScope,
    shell::TitleBar,
    theme::{ActiveTheme, Radius},
    typography::{Caption, Heading, Label, Paragraph},
};
#[cfg(target_os = "windows")]
use ely_core::{buttons::IconButton, primitives::IconName};
use gpui::{
    AnyElement, App, AppContext, Context, Entity, FocusHandle, InteractiveElement, IntoElement,
    ParentElement, Render, StatefulInteractiveElement, Styled, Subscription, Window, div,
    prelude::FluentBuilder as _, px, relative,
};
use lexvault_core::{DataManager, DataPaths, Entry, LexVault, SearchCursor, Suggestion};

use crate::{SelectNext, SelectPrevious};

pub(crate) const KEY_CONTEXT: &str = "LexVault";
pub(crate) const SEARCH_CONTEXT: &str = "ElyInput";
const PAGE_SIZE: usize = 80;
const MAX_HISTORY: usize = 20;

pub(crate) struct LexVaultGui {
    focus: FocusHandle,
    search: Entity<TextInput>,
    dictionary: Option<LexVault>,
    expected_database: String,
    setup_error: Option<String>,
    query: String,
    items: Vec<Suggestion>,
    cursor: Option<SearchCursor>,
    exhausted: bool,
    selected: usize,
    entry: Option<Entry>,
    status: String,
    history: Vec<String>,
    #[cfg(target_os = "windows")]
    always_on_top: bool,
    _search_events: Subscription,
}

impl LexVaultGui {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| {
            TextInput::new(window, cx)
                .placeholder("输入英文或中文，例如 kiss、苹果")
                .label("搜索词典")
        });
        let search_events = cx.subscribe(&search, |this, _, event, cx| match event {
            InputEvent::Changed => this.refresh(cx),
            InputEvent::Submit => this.remember_query(cx),
            InputEvent::Focus | InputEvent::Blur => {}
        });

        let mut this = Self {
            focus: cx.focus_handle(),
            search,
            dictionary: None,
            expected_database: String::new(),
            setup_error: None,
            query: String::new(),
            items: Vec::new(),
            cursor: None,
            exhausted: false,
            selected: 0,
            entry: None,
            status: String::new(),
            history: Vec::new(),
            #[cfg(target_os = "windows")]
            always_on_top: false,
            _search_events: search_events,
        };
        this.open_dictionary();
        window.focus(&this.search.read(cx).focus().clone(), cx);
        this
    }

    fn open_dictionary(&mut self) {
        let paths = DataPaths::from_env();
        self.expected_database = paths.database.display().to_string();
        let manager = DataManager::new(paths);
        match manager.ensure_ready(|_| {}) {
            Ok(path) => match LexVault::open(path) {
                Ok(dictionary) => {
                    self.dictionary = Some(dictionary);
                    self.setup_error = None;
                    self.status.clear();
                }
                Err(error) => {
                    self.dictionary = None;
                    self.setup_error = Some(error.to_string());
                }
            },
            Err(error) => {
                self.dictionary = None;
                self.setup_error = Some(error.to_string());
            }
        }
    }

    fn reload_dictionary(&mut self, cx: &mut Context<Self>) {
        self.dictionary = None;
        self.open_dictionary();
        if self.dictionary.is_some() {
            self.refresh(cx);
        } else {
            cx.notify();
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let query = self.search.read(cx).text().trim().to_owned();
        let changed = query != self.query;
        self.query = query;
        self.items.clear();
        self.cursor = None;
        self.exhausted = false;
        self.selected = 0;
        self.entry = None;
        self.status.clear();

        if self.dictionary.is_none() || self.query.is_empty() {
            self.exhausted = self.query.is_empty();
            cx.notify();
            return;
        }

        self.load_more();
        if !self.items.is_empty() {
            self.select_index(0, cx);
        }
        if changed {
            cx.notify();
        }
    }

    fn load_more(&mut self) {
        if self.exhausted {
            return;
        }
        let (Some(dictionary), false) = (&self.dictionary, self.query.is_empty()) else {
            self.exhausted = true;
            return;
        };

        match dictionary.search_page(&self.query, self.cursor.as_ref(), PAGE_SIZE) {
            Ok(page) => {
                let before = self.items.len();
                self.items.extend(page.items);
                self.cursor = page.next_cursor;
                self.exhausted = page.exhausted;
                if self.items.len() == before && !page.exhausted {
                    self.status = "分页查询没有取得新词条".to_owned();
                    self.exhausted = true;
                }
            }
            Err(error) => {
                self.status = error.to_string();
                self.exhausted = true;
            }
        }
    }

    fn select_index(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(suggestion) = self.items.get(index) else {
            return;
        };
        let id = suggestion.id;
        self.selected = index;
        self.status.clear();
        self.entry = self
            .dictionary
            .as_ref()
            .and_then(|dictionary| dictionary.lookup_by_id(id).ok().flatten());
        if self.entry.is_none() {
            self.status = format!("无法读取词条：{}", suggestion.word);
        }
        cx.notify();
    }

    fn move_down(&mut self, cx: &mut Context<Self>) {
        if self.items.is_empty() {
            return;
        }
        if self.selected + 1 >= self.items.len() {
            self.load_more();
        }
        if self.selected + 1 < self.items.len() {
            self.select_index(self.selected + 1, cx);
        } else {
            cx.notify();
        }
    }

    fn move_up(&mut self, cx: &mut Context<Self>) {
        if self.selected > 0 {
            self.select_index(self.selected - 1, cx);
        }
    }

    fn remember_query(&mut self, cx: &mut Context<Self>) {
        let query = self.search.read(cx).text().trim().to_owned();
        if query.is_empty() {
            return;
        }
        self.history.retain(|item| item != &query);
        self.history.insert(0, query);
        self.history.truncate(MAX_HISTORY);
        cx.notify();
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.move_down(cx);
    }

    fn select_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        self.move_up(cx);
    }

    #[cfg(target_os = "windows")]
    fn toggle_always_on_top(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let next = !self.always_on_top;
        if set_always_on_top(window, next) {
            self.always_on_top = next;
            self.status.clear();
        } else {
            self.status = "无法切换窗口置顶状态".to_owned();
        }
        cx.notify();
    }

    fn render_setup(&self, cx: &mut Context<Self>) -> AnyElement {
        let retry = cx.entity();
        let error = self
            .setup_error
            .clone()
            .unwrap_or_else(|| "词典数据库尚未就绪。".to_owned());
        let theme = cx.theme();
        ScrollArea::new("setup-scroll")
            .vertical()
            .size_full()
            .child(
                Page::new("安装词典数据")
                    .subtitle("LexVault 不随安装包分发有道词典数据")
                    .child(
                        Card::new()
                            .header(CardHeader::new("缺少 youdao.db"))
                            .child(Paragraph::new(error).text_color(theme.colors.fg_muted))
                            .child(Label::new("目标路径").text_color(theme.colors.fg_muted))
                            .child(
                                div()
                                    .p_3()
                                    .rounded(theme.radius(Radius::Md))
                                    .bg(theme.colors.sunken)
                                    .font_family("JetBrains Mono")
                                    .child(self.expected_database.clone()),
                            )
                            .footer(Button::new("retry-data", "重新检查").primary().on_click(
                                move |_, _, cx| {
                                    retry.update(cx, |this, cx| this.reload_dictionary(cx));
                                },
                            )),
                    ),
            )
            .into_any_element()
    }

    fn render_candidates(&self, cx: &mut Context<Self>) -> AnyElement {
        let title = if self.items.is_empty() {
            "预选".to_owned()
        } else if self.exhausted {
            format!("预选 · {}", self.items.len())
        } else {
            format!("预选 · {}+", self.items.len())
        };
        let theme = cx.theme();
        let list = if self.query.is_empty() {
            div().flex_1().min_h_0().into_any_element()
        } else if self.items.is_empty() {
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .items_center()
                .justify_center()
                .p_6()
                .child(Caption::new("没有匹配词条").text_color(cx.theme().colors.fg_subtle))
                .into_any_element()
        } else {
            let app = cx.entity();
            let current = self.selected;
            let rows = self
                .items
                .iter()
                .enumerate()
                .map(|(index, suggestion)| {
                    let click = app.clone();
                    let row = ListItem::new(
                        ("suggestion", suggestion.id as u64),
                        suggestion.word.clone(),
                    )
                    .selected(index == current);
                    let row = if suggestion.brief.is_empty() {
                        row
                    } else {
                        row.description(suggestion.brief.clone())
                    };
                    row.on_click(move |_, _, cx| {
                        click.update(cx, |this, cx| this.select_index(index, cx));
                    })
                })
                .collect::<Vec<_>>();
            let more = app;
            div()
                .id("candidate-scroll")
                .flex_1()
                .w_full()
                .min_h_0()
                .overflow_y_scroll()
                .child(
                    div().p_1().child(
                        InfiniteList::new("candidate-pages")
                            .children(rows)
                            .done(self.exhausted)
                            .on_more(move |_, cx| {
                                more.update(cx, |this, cx| {
                                    this.load_more();
                                    cx.notify();
                                });
                            }),
                    ),
                )
                .into_any_element()
        };

        div()
            .w(px(340.0))
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(theme.radius(Radius::Md))
            .border_1()
            .border_color(theme.colors.border)
            .bg(theme.colors.surface)
            .child(
                div()
                    .flex_none()
                    .h(px(38.0))
                    .px_3()
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(theme.colors.border)
                    .text_size(theme.text_size(ely_core::theme::TextSize::Sm))
                    .text_color(theme.colors.fg_muted)
                    .child(title),
            )
            .child(list)
            .into_any_element()
    }

    fn render_definition(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let content = if let Some(entry) = &self.entry {
            let mut blocks = Vec::new();
            blocks.push(
                div()
                    .flex()
                    .items_end()
                    .gap_3()
                    .child(Heading::h2(entry.word.clone()))
                    .children(entry.phonetic.as_ref().map(|phonetic| {
                        Caption::new(format!("/{phonetic}/")).text_color(theme.colors.fg_muted)
                    }))
                    .into_any_element(),
            );

            if let Some(translation) = non_empty(entry.translation.as_deref()) {
                blocks.push(text_content(translation, cx));
            }
            if let Some(definition) = non_empty(entry.definition.as_deref()) {
                blocks.push(text_content(definition, cx));
            }
            if let Some(exchange) = non_empty(entry.exchange.as_deref()) {
                blocks.push(
                    Caption::new(format!("词形：{exchange}"))
                        .text_color(theme.colors.fg_muted)
                        .into_any_element(),
                );
            }
            if blocks.len() == 1 {
                blocks.push(
                    Caption::new("该词条没有可显示的释义")
                        .text_color(theme.colors.fg_muted)
                        .into_any_element(),
                );
            }
            div()
                .flex()
                .flex_col()
                .gap_5()
                .p_5()
                .children(blocks)
                .into_any_element()
        } else {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .p_6()
                .child(Caption::new("选择一个候选词条查看释义").text_color(theme.colors.fg_subtle))
                .into_any_element()
        };

        Panel::new("释义")
            .flex_1()
            .h_full()
            .min_h_0()
            .child(
                ScrollArea::new("definition-scroll")
                    .vertical()
                    .size_full()
                    .child(content),
            )
            .into_any_element()
    }
}

impl Render for LexVaultGui {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors.clone();
        let setup = self.dictionary.is_none();
        let history = self.history.clone();
        let search = SearchInput::new("lexvault-search", &self.search).history(history);
        let body = if setup {
            self.render_setup(cx)
        } else {
            div()
                .size_full()
                .flex()
                .gap_4()
                .p_4()
                .child(
                    div()
                        .w(px(340.0))
                        .h_full()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(search)
                        .when(!self.status.is_empty(), |left| {
                            left.child(Caption::new(self.status.clone()).text_color(colors.danger))
                        })
                        .child(self.render_candidates(cx)),
                )
                .child(self.render_definition(cx))
                .into_any_element()
        };

        #[allow(unused_mut)]
        let mut title_bar = TitleBar::new("lexvault-titlebar").title("LexVault");
        #[cfg(target_os = "windows")]
        {
            let app = cx.entity();
            let always_on_top = self.always_on_top;
            title_bar = title_bar.action(
                IconButton::new(
                    "always-on-top",
                    if always_on_top {
                        IconName::PinOff
                    } else {
                        IconName::Pin
                    },
                )
                .tooltip(if always_on_top {
                    "取消置顶"
                } else {
                    "窗口置顶"
                })
                .on_click(move |_, window, cx| {
                    app.update(cx, |this, cx| this.toggle_always_on_top(window, cx));
                }),
            );
        }

        div()
            .size_full()
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .child(
                FocusScope::new(&self.focus)
                    .root()
                    .size_full()
                    .flex()
                    .flex_col()
                    .bg(colors.bg)
                    .text_color(colors.fg)
                    .child(title_bar)
                    .child(div().flex_1().min_h_0().child(body)),
            )
    }
}

#[cfg(target_os = "windows")]
fn set_always_on_top(window: &Window, enabled: bool) -> bool {
    use raw_window_handle::RawWindowHandle;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos,
    };

    let Ok(handle) = raw_window_handle::HasWindowHandle::window_handle(window) else {
        return false;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return false;
    };
    let insert_after = if enabled {
        HWND_TOPMOST
    } else {
        HWND_NOTOPMOST
    };
    let hwnd = handle.hwnd.get() as *mut core::ffi::c_void;
    unsafe {
        SetWindowPos(
            hwnd,
            insert_after,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        ) != 0
    }
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn text_content(text: &str, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let lines = text
        .lines()
        .map(|line| {
            if line.trim().is_empty() {
                div().h(px(6.0)).into_any_element()
            } else {
                Paragraph::new(line.to_owned()).into_any_element()
            }
        })
        .collect::<Vec<_>>();
    div()
        .flex()
        .flex_col()
        .gap_1()
        .line_height(relative(1.6))
        .text_color(theme.colors.fg)
        .children(lines)
        .into_any_element()
}
