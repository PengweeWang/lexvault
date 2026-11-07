use std::{
    sync::mpsc::{self, Sender},
    time::Duration,
};

#[cfg(target_os = "windows")]
use ely_core::{buttons::IconButton, primitives::IconName};
use ely_core::{
    buttons::{Button, CopyButton},
    forms::{InputEvent, SearchInput, TextInput},
    layout::{Card, CardHeader, Page, Panel, ScrollArea},
    lists::{InfiniteList, ListItem},
    primitives::FocusScope,
    shell::TitleBar,
    theme::{ActiveTheme, ControlSize, Radius, TextSize},
    typography::{Caption, Heading, Label, Paragraph, SelectableText},
};
use gpui::{
    AnyElement, App, AppContext, Context, Entity, FocusHandle, InteractiveElement, IntoElement,
    ParentElement, Render, StatefulInteractiveElement, Styled, Subscription, Task, Window, div,
    prelude::FluentBuilder as _, px, relative,
};
use lexvault_core::{DataManager, DataPaths, Entry, LexVault, SearchCursor, Suggestion};

use crate::{
    SelectNext, SelectPrevious,
    worker::{DictionaryWorker, Request, Response},
};

pub(crate) const KEY_CONTEXT: &str = "LexVault";
pub(crate) const SEARCH_CONTEXT: &str = "ElyInput";
const PAGE_SIZE: usize = 40;
const MAX_HISTORY: usize = 20;

pub(crate) struct LexVaultGui {
    focus: FocusHandle,
    search: Entity<TextInput>,
    worker: Option<DictionaryWorker>,
    responses: Sender<Response>,
    expected_database: String,
    setup_error: Option<String>,
    query: String,
    query_epoch: u64,
    selection_epoch: u64,
    loading_search: bool,
    loading_more: bool,
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
    _worker_events: Task<()>,
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
        let (responses, response_events) = mpsc::channel();
        let worker_events = cx.spawn_in(window, async move |this, cx| {
            loop {
                let incoming = response_events.try_iter().collect::<Vec<_>>();
                if !incoming.is_empty() {
                    let updated = cx.update(|_, cx| {
                        this.update(cx, |this, cx| this.handle_responses(incoming, cx))
                    });
                    if updated.and_then(|inner| inner).is_err() {
                        return;
                    }
                }
                cx.background_executor()
                    .timer(Duration::from_millis(8))
                    .await;
            }
        });

        let mut this = Self {
            focus: cx.focus_handle(),
            search,
            worker: None,
            responses,
            expected_database: String::new(),
            setup_error: None,
            query: String::new(),
            query_epoch: 0,
            selection_epoch: 0,
            loading_search: false,
            loading_more: false,
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
            _worker_events: worker_events,
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
                Ok(dictionary) => match DictionaryWorker::spawn(dictionary, self.responses.clone())
                {
                    Ok(worker) => {
                        self.worker = Some(worker);
                        self.setup_error = None;
                        self.status.clear();
                    }
                    Err(error) => {
                        self.worker = None;
                        self.setup_error = Some(format!("无法启动词典查询线程：{error}"));
                    }
                },
                Err(error) => {
                    self.worker = None;
                    self.setup_error = Some(error.to_string());
                }
            },
            Err(error) => {
                self.worker = None;
                self.setup_error = Some(error.to_string());
            }
        }
    }

    fn reload_dictionary(&mut self, cx: &mut Context<Self>) {
        self.worker = None;
        self.open_dictionary();
        if self.worker.is_some() {
            self.refresh(cx);
        } else {
            cx.notify();
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let query = self.search.read(cx).text().trim().to_owned();
        self.query = query;
        self.query_epoch = self.query_epoch.wrapping_add(1);
        self.selection_epoch = self.selection_epoch.wrapping_add(1);
        self.loading_search = false;
        self.loading_more = false;
        self.items.clear();
        self.cursor = None;
        self.exhausted = false;
        self.selected = 0;
        self.entry = None;
        self.status.clear();

        if self.worker.is_none() || self.query.is_empty() {
            self.exhausted = self.query.is_empty();
            cx.notify();
            return;
        }

        let sent = self.worker.as_ref().is_some_and(|worker| {
            worker.send(Request::Search {
                query_epoch: self.query_epoch,
                query: self.query.clone(),
                cursor: None,
                page_size: PAGE_SIZE,
                append: false,
            })
        });
        if sent {
            self.loading_search = true;
        } else {
            self.status = "无法提交词典查询".to_owned();
            self.exhausted = true;
        }
        cx.notify();
    }

    fn load_more(&mut self, cx: &mut Context<Self>) {
        if self.exhausted || self.loading_search || self.loading_more {
            return;
        }
        if self.worker.is_none() || self.query.is_empty() {
            self.exhausted = true;
            return;
        }

        let sent = self.worker.as_ref().is_some_and(|worker| {
            worker.send(Request::Search {
                query_epoch: self.query_epoch,
                query: self.query.clone(),
                cursor: self.cursor.clone(),
                page_size: PAGE_SIZE,
                append: true,
            })
        });
        if sent {
            self.loading_more = true;
        } else {
            self.status = "无法提交分页查询".to_owned();
            self.exhausted = true;
        }
        cx.notify();
    }

    fn select_index(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(suggestion) = self.items.get(index) else {
            return;
        };
        let id = suggestion.id;
        self.selected = index;
        self.selection_epoch = self.selection_epoch.wrapping_add(1);
        self.status.clear();
        let word = suggestion.word.clone();
        let sent = self.worker.as_ref().is_some_and(|worker| {
            worker.send(Request::Lookup {
                selection_epoch: self.selection_epoch,
                id,
            })
        });
        if sent {
            self.entry = None;
        } else {
            self.entry = None;
            self.status = format!("无法读取词条：{word}");
        }
        cx.notify();
    }

    fn move_down(&mut self, cx: &mut Context<Self>) {
        if self.items.is_empty() {
            return;
        }
        if self.selected + 1 >= self.items.len() {
            self.load_more(cx);
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

    fn handle_responses(&mut self, responses: Vec<Response>, cx: &mut Context<Self>) {
        let mut notify = false;
        for response in responses {
            match response {
                Response::Search {
                    query_epoch,
                    append,
                    result,
                } => {
                    if query_epoch != self.query_epoch {
                        continue;
                    }
                    if append {
                        self.loading_more = false;
                    } else {
                        self.loading_search = false;
                    }
                    match result {
                        Ok(page) => {
                            if append {
                                let before = self.items.len();
                                self.items.extend(page.items);
                                self.cursor = page.next_cursor;
                                self.exhausted = page.exhausted;
                                if self.items.len() == before && !page.exhausted {
                                    self.status = "分页查询没有取得新词条".to_owned();
                                    self.exhausted = true;
                                }
                            } else {
                                self.items = page.items;
                                self.cursor = page.next_cursor;
                                self.exhausted = page.exhausted;
                                self.selected = 0;
                                self.entry = None;
                                if !self.items.is_empty() {
                                    self.select_index(0, cx);
                                }
                            }
                        }
                        Err(error) => {
                            self.status = error;
                            self.exhausted = true;
                        }
                    }
                }
                Response::Lookup {
                    selection_epoch,
                    result,
                } => {
                    if selection_epoch != self.selection_epoch {
                        continue;
                    }
                    match result {
                        Ok(Some(entry)) => {
                            self.entry = Some(entry);
                            self.status.clear();
                        }
                        Ok(None) => {
                            self.entry = None;
                            self.status = "无法读取所选词条".to_owned();
                        }
                        Err(error) => {
                            self.entry = None;
                            self.status = error;
                        }
                    }
                }
            }
            notify = true;
        }
        if notify {
            cx.notify();
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
                                    this.load_more(cx);
                                    cx.notify();
                                });
                            }),
                    ),
                )
                .into_any_element()
        };

        div()
            .w(px(280.0))
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
                    .h(px(30.0))
                    .px_2()
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(theme.colors.border)
                    .text_size(theme.text_size(TextSize::Xs))
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
                    .gap_2()
                    .child(Heading::h3(entry.word.clone()))
                    .children(entry.phonetic.as_ref().map(|phonetic| {
                        Caption::new(format!("/{phonetic}/")).text_color(theme.colors.fg_muted)
                    }))
                    .into_any_element(),
            );

            if let Some(translation) = non_empty(entry.translation.as_deref()) {
                blocks.push(selectable_content(
                    ("definition-translation", entry.id as u64),
                    translation,
                    cx,
                ));
            }
            if let Some(definition) = non_empty(entry.definition.as_deref()) {
                blocks.push(selectable_content(
                    ("definition-body", entry.id as u64),
                    definition,
                    cx,
                ));
            }
            if let Some(exchange) = non_empty(entry.exchange.as_deref()) {
                blocks.push(selectable_content(
                    ("definition-forms", entry.id as u64),
                    &format!("词形：{exchange}"),
                    cx,
                ));
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
                .gap_3()
                .p_3()
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

        let mut panel = Panel::new("释义").flex_1().h_full().min_h_0();
        if let Some(entry) = &self.entry {
            panel = panel.action(CopyButton::new(
                ("copy-definition", entry.id as u64),
                entry_copy_text(entry),
            ));
        }
        panel
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
        let setup = self.worker.is_none();
        let history = self.history.clone();
        let search = SearchInput::new("lexvault-search", &self.search)
            .history(history)
            .size(ControlSize::Sm);
        let body = if setup {
            self.render_setup(cx)
        } else {
            div()
                .size_full()
                .flex()
                .gap_2()
                .p_2()
                .child(
                    div()
                        .w(px(280.0))
                        .h_full()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .gap_2()
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

fn selectable_content(id: impl Into<gpui::ElementId>, text: &str, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .flex()
        .flex_col()
        .line_height(relative(1.5))
        .text_size(theme.text_size(TextSize::Base))
        .text_color(theme.colors.fg)
        .child(SelectableText::new(id, text.to_owned()))
        .into_any_element()
}

fn entry_copy_text(entry: &Entry) -> String {
    let mut text = entry.word.clone();
    if let Some(phonetic) = non_empty(entry.phonetic.as_deref()) {
        text.push_str(&format!("\n/{phonetic}/"));
    }
    if let Some(translation) = non_empty(entry.translation.as_deref()) {
        text.push_str("\n\n");
        text.push_str(translation);
    }
    if let Some(definition) = non_empty(entry.definition.as_deref()) {
        text.push_str("\n\n");
        text.push_str(definition);
    }
    if let Some(exchange) = non_empty(entry.exchange.as_deref()) {
        text.push_str("\n\n词形：");
        text.push_str(exchange);
    }
    text
}

#[cfg(test)]
mod tests {
    use lexvault_core::Entry;

    use super::entry_copy_text;

    #[test]
    fn copied_entry_contains_readable_definition_text() {
        let entry = Entry {
            id: 1,
            word: "catabolize".to_owned(),
            phonetic: Some("kə'tæbəlaiz".to_owned()),
            definition: None,
            translation: Some("使发生分解代谢".to_owned()),
            pos: None,
            collins: 0,
            oxford: 0,
            tags: None,
            bnc: None,
            frq: None,
            exchange: Some("过去式：catabolized".to_owned()),
            detail: None,
            audio: None,
        };
        assert_eq!(
            entry_copy_text(&entry),
            "catabolize\n/kə'tæbəlaiz/\n\n使发生分解代谢\n\n词形：过去式：catabolized"
        );
    }
}
