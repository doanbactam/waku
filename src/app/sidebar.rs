use gpui::{KeyBinding, actions};
use std::collections::HashSet;

use super::*;

actions!(waku_sidebar, [CancelSessionRename]);

const SESSION_RENAME_PARENT_CONTEXT: &str = "SessionRename";
const SESSION_RENAME_FIELD_CONTEXT: &str = "SessionRename > ComposerInput";

/// Keep Escape inside the focused inline editor so it cancels the rename,
/// rather than falling through to the window-wide Stop action.
pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(
        "escape",
        CancelSessionRename,
        Some(SESSION_RENAME_FIELD_CONTEXT),
    )]);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SidebarProjectFilter {
    Project(Uuid),
    Projectless,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SidebarStatusFilter {
    All,
    Active,
    NeedsYou,
    Done,
    Failed,
}

impl SidebarStatusFilter {
    fn label(self) -> String {
        match self {
            Self::All => tr!("sidebar.all_statuses"),
            Self::Active => tr!("sidebar.active"),
            Self::NeedsYou => tr!("sidebar.needs_you"),
            Self::Done => tr!("sidebar.done"),
            Self::Failed => tr!("sidebar.failed"),
        }
    }

    fn matches(self, status: SessionStatus) -> bool {
        match self {
            Self::All => true,
            Self::Active => matches!(status, SessionStatus::Connecting | SessionStatus::Working),
            Self::NeedsYou => status == SessionStatus::Waiting,
            Self::Done => status == SessionStatus::Idle,
            Self::Failed => status == SessionStatus::Failed,
        }
    }
}

/// Which active filter a removable chip in the sidebar stands for.
#[derive(Clone, Copy)]
enum SidebarFilterChipKind {
    Provider,
    Status,
}

fn session_group_header(theme: &Theme) -> Div {
    div()
        .h(px(28.0))
        .px(px(8.0))
        .flex()
        .items_center()
        .text_size(px(12.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.text_tertiary)
}

fn updater_button_available_content(
    foreground: Hsla,
    label: SharedString,
    label_reveal: f32,
) -> Div {
    div()
        .relative()
        .size_full()
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .opacity(1.0 - label_reveal)
                .child(icon("icons/download.svg", 12.0, foreground)),
        )
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .whitespace_nowrap()
                .opacity(label_reveal)
                .child(label),
        )
}

/// Height of a session card plus the separation reserved beneath it in the
/// virtualized sidebar list. Keep the gap inside the list row so measured and
/// estimated heights stay identical for off-screen sessions.
const SIDEBAR_SESSION_CARD_HEIGHT: f32 = 51.0;
const SIDEBAR_SESSION_ROW_GAP: f32 = 1.0;
const SIDEBAR_SESSION_ROW_HEIGHT: f32 = SIDEBAR_SESSION_CARD_HEIGHT + SIDEBAR_SESSION_ROW_GAP;
const SIDEBAR_ACTION_ROW_HEIGHT: f32 = 32.0;

/// The session row's trailing time: how long the live turn has been working,
/// or how long ago the agent last replied. A session that has never replied
/// shows nothing.
pub(super) fn session_time_label(session: &AgentSession, now: u64) -> Option<String> {
    if session.is_busy()
        && let Some(turn) = session
            .turns
            .last()
            .filter(|turn| turn.status == TurnStatus::Running)
    {
        return Some(tr!(
            "sidebar.working",
            elapsed = format_working_elapsed(now.saturating_sub(turn.started_at))
        ));
    }
    session
        .last_reply_at
        .map(|last_reply_at| format_time_ago(now.saturating_sub(last_reply_at)))
}

/// Recency for sidebar ordering. A submitted turn promotes the
/// task immediately, while metadata edits such as a rename do not; a task with
/// no turns stays anchored to when it was created.
fn sidebar_session_timestamp(session: &AgentSession) -> u64 {
    session.last_reply_at.unwrap_or(session.created_at)
}

fn sidebar_session_matches_project_filter(
    session: &AgentSession,
    project_filter: Option<SidebarProjectFilter>,
    provider_filter: Option<ProviderKind>,
    projectless_project_ids: &HashSet<Uuid>,
) -> bool {
    if !session.has_started()
        || provider_filter.is_some_and(|provider| session.provider != provider)
    {
        return false;
    }
    match project_filter {
        None => true,
        Some(SidebarProjectFilter::Project(project_id)) => session.project_id == project_id,
        Some(SidebarProjectFilter::Projectless) => {
            projectless_project_ids.contains(&session.project_id)
        }
    }
}

fn collect_projectless_project_ids(projects: &[Project]) -> HashSet<Uuid> {
    projects
        .iter()
        .filter(|project| project.is_projectless())
        .map(|project| project.id)
        .collect()
}

/// Compact "how long ago" for the sidebar: "just now", then one coarse unit —
/// "5m", "3h", "420d". Days are the largest unit so a glance still reads as a
/// count rather than a date.
pub(super) fn format_time_ago(seconds: u64) -> String {
    match seconds {
        0..=59 => tr!("sidebar.just_now"),
        60..=3_599 => tr!("sidebar.minutes_ago", count = seconds / 60),
        3_600..=86_399 => tr!("sidebar.hours_ago", count = seconds / 3_600),
        _ => tr!("sidebar.days_ago", count = seconds / 86_400),
    }
}

/// One row of the virtualized sidebar session history.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SidebarRow {
    /// Project header for the project-first task list, plus how many matching
    /// sessions the group shows under the current filters.
    ProjectHeader(Uuid, usize),
    /// A started session.
    Session(Uuid),
    /// Spacing between project groups.
    GroupSpacer,
    /// History exists, but the project scope hides every task.
    EmptyFilter,
}

impl Waku {
    pub(super) fn window_drag_region(
        &self,
        region: Stateful<Div>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        region
            .window_control_area(WindowControlArea::Drag)
            .on_click(|event, window, _| {
                if event.click_count() == 2 {
                    crate::platform::titlebar_double_click(window);
                }
            })
            .on_mouse_down_out(cx.listener(|this, _, _, _| {
                this.header_drag_armed = false;
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    this.header_drag_armed = true;
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    this.header_drag_armed = false;
                }),
            )
            .on_mouse_move(cx.listener(|this, _, window, _| {
                if this.header_drag_armed {
                    this.header_drag_armed = false;
                    crate::platform::start_window_move(window);
                }
            }))
    }
    // ── Sidebar ────────────────────────────────────────────────────────────

    fn render_fps_counter(&self, cx: &mut Context<Self>) -> Div {
        let theme = Theme::current(cx);
        let fps = self.fps_value;
        let dot = if fps == 0 {
            theme.text_ghost
        } else if fps >= 55 {
            theme.success
        } else if fps >= 30 {
            theme.warning
        } else {
            theme.danger
        };
        div()
            .flex_none()
            .h(px(26.0))
            .px(px(6.0))
            .flex()
            .items_center()
            .gap(px(5.0))
            .text_size(px(11.0))
            .line_height(px(0.0))
            .child(div().w(px(6.0)).h(px(6.0)).rounded_full().bg(dot))
            .child(
                div()
                    .text_color(theme.text_tertiary)
                    .font_family(crate::md::render::MONO_FAMILY)
                    .child(SharedString::from(format!("{fps} FPS"))),
            )
    }

    fn render_sidebar_toggle(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let theme = Theme::current(cx);
        div()
            .id("toggle-sidebar")
            .w(px(26.0))
            .h(px(26.0))
            .flex_none()
            .rounded(px(6.0))
            .flex()
            .items_center()
            .justify_center()
            .cursor_default()
            .hover(|element| element.bg(theme.overlay))
            .active(|element| element.bg(theme.overlay_strong))
            .child(icon("icons/panel-left.svg", 14.0, theme.text_tertiary))
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .on_click(cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.set_sidebar_visible(!this.sidebar_visible, cx);
            }))
    }

    pub(super) fn render_history_button(
        &self,
        id: &'static str,
        icon_path: &'static str,
        enabled: bool,
        navigate_back: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = Theme::current(cx);
        div()
            .id(id)
            .w(px(26.0))
            .h(px(26.0))
            .flex_none()
            .rounded(px(6.0))
            .flex()
            .items_center()
            .justify_center()
            .cursor_default()
            .when(!enabled, |element| element.opacity(0.35))
            .when(enabled, |element| {
                element
                    .hover(|element| element.bg(theme.overlay))
                    .active(|element| element.bg(theme.overlay_strong))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        if navigate_back {
                            this.navigate_back_action(&NavigateBack, window, cx);
                        } else {
                            this.navigate_forward_action(&NavigateForward, window, cx);
                        }
                    }))
            })
            .child(icon(icon_path, 14.0, theme.text_tertiary))
    }

    fn render_sidebar_titlebar(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id("sidebar-titlebar")
            .h(px(48.0))
            .flex_none()
            .flex()
            .items_center()
            .child(
                self.window_drag_region(
                    div()
                        .id("sidebar-traffic-light-drag-region")
                        .w(px(TRAFFIC_LIGHT_CLEARANCE))
                        .h_full()
                        .flex_none(),
                    cx,
                ),
            )
            .child(self.render_sidebar_toggle(cx))
            .child(
                div()
                    .ml(px(6.0))
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .child(self.render_history_button(
                        "navigate-back",
                        "icons/arrow-left.svg",
                        !self.session_navigation.back.is_empty(),
                        true,
                        cx,
                    ))
                    .child(self.render_history_button(
                        "navigate-forward",
                        "icons/arrow-right.svg",
                        !self.session_navigation.forward.is_empty(),
                        false,
                        cx,
                    )),
            )
            .child(self.window_drag_region(
                div().id("sidebar-titlebar-drag-region").h_full().flex_1(),
                cx,
            ))
    }

    fn render_sidebar_project_action(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let theme = Theme::current(cx);
        div()
            .id("add-project")
            .w(px(26.0))
            .h(px(26.0))
            .flex_none()
            .rounded(px(6.0))
            .flex()
            .items_center()
            .justify_center()
            .cursor_default()
            .hover(|element| element.bg(theme.overlay))
            .active(|element| element.bg(theme.overlay_strong))
            .tooltip(Tooltip::text(tr_cow!("project.new_project")))
            .child(icon("icons/folder-new.svg", 14.0, theme.text_tertiary))
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .on_click(cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.add_project(cx);
            }))
    }

    fn render_sidebar_action_row(
        &self,
        id: &'static str,
        icon_path: &'static str,
        label: String,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = Theme::current(cx);
        div()
            .id(id)
            .tab_index(0)
            .w_full()
            .h(px(SIDEBAR_ACTION_ROW_HEIGHT))
            .flex_none()
            .px(px(4.0))
            .rounded(px(7.0))
            .flex()
            .items_center()
            .gap(px(10.0))
            .cursor_default()
            .focus_visible(|style| style.border_1().border_color(theme.accent))
            .hover(|element| element.bg(theme.sidebar_item_background))
            .active(|element| element.bg(theme.overlay_strong))
            .child(
                div()
                    .size(px(20.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(icon_path, 16.0, theme.text_secondary)),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.0))
                    .text_color(theme.text_secondary)
                    .child(label),
            )
    }

    fn render_sidebar_chrome(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex_none()
            .w_full()
            .px(px(10.0))
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(self.render_sidebar_project_selector(cx))
                    .child(self.render_sidebar_project_action(cx)),
            )
            .child(self.render_sidebar_new_session(cx))
            .child(self.render_sidebar_search(cx))
            .child(self.render_sidebar_filters(cx))
            .when_some(self.render_sidebar_filter_chips(cx), |element, chips| {
                element.child(chips)
            })
            .pb(px(8.0))
    }

    fn sidebar_project_filter_label(&self) -> String {
        match self.resolved_sidebar_project_filter() {
            None => tr!("sidebar.all_projects"),
            Some(SidebarProjectFilter::Project(project_id)) => self
                .state
                .projects
                .iter()
                .find(|project| project.id == project_id)
                .map(Project::display_name)
                .unwrap_or_else(|| tr!("sidebar.all_projects")),
            Some(SidebarProjectFilter::Projectless) => tr!("project.no_project_name"),
        }
    }

    fn sidebar_active_filter_count(&self) -> usize {
        usize::from(self.sidebar_provider_filter.is_some())
            + usize::from(self.sidebar_status_filter != SidebarStatusFilter::All)
    }

    fn clear_sidebar_filter_chip(&mut self, kind: SidebarFilterChipKind, cx: &mut Context<Self>) {
        match kind {
            SidebarFilterChipKind::Provider => self.set_sidebar_provider_filter(None, cx),
            SidebarFilterChipKind::Status => {
                self.sidebar_status_filter = SidebarStatusFilter::All;
                cx.notify();
            }
        }
    }

    fn resolved_sidebar_project_filter(&self) -> Option<SidebarProjectFilter> {
        match self.sidebar_project_filter {
            Some(SidebarProjectFilter::Project(project_id)) => self
                .state
                .projects
                .iter()
                .any(|project| project.id == project_id && !project.is_projectless())
                .then_some(SidebarProjectFilter::Project(project_id)),
            Some(SidebarProjectFilter::Projectless) => self
                .state
                .projects
                .iter()
                .any(Project::is_projectless)
                .then_some(SidebarProjectFilter::Projectless),
            None => None,
        }
    }

    fn render_sidebar_project_selector(&self, cx: &mut Context<Self>) -> Div {
        let theme = Theme::current(cx);
        let scoped = self.resolved_sidebar_project_filter().is_some();
        let handle = self.menu_handle("sidebar-project-filter", cx);
        let weak = cx.entity().downgrade();
        let trigger = div()
            .id("sidebar-project-filter")
            .min_w_0()
            .h(px(SIDEBAR_ACTION_ROW_HEIGHT))
            .flex_1()
            .px(px(4.0))
            .rounded(px(7.0))
            .flex()
            .items_center()
            .gap(px(10.0))
            .cursor_default()
            .focus_visible(|style| style.border_1().border_color(theme.accent))
            .hover(|element| element.bg(theme.sidebar_item_background))
            .when(handle.is_open(), |element| {
                element.bg(theme.sidebar_item_background)
            })
            .child(
                div()
                    .size(px(20.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(
                        if scoped { "icons/folder.svg" } else { "icons/globe.svg" },
                        16.0,
                        theme.text_secondary,
                    )),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_size(px(13.0))
                    .text_color(if scoped {
                        theme.text
                    } else {
                        theme.text_secondary
                    })
                    .child(self.sidebar_project_filter_label()),
            )
            .child(icon("icons/chevron-down.svg", 11.0, theme.text_ghost));
        let picker = dropdown_menu(
            trigger,
            "sidebar-project-filter-menu",
            &handle,
            MenuAlign::BelowLeft,
            move |cx| {
                let Some(entity) = weak.upgrade() else {
                    return Vec::new();
                };
                let this = entity.read(cx);
                let selected = this.resolved_sidebar_project_filter();
                let project_options = this
                    .state
                    .projects
                    .iter()
                    .filter(|project| !project.is_projectless())
                    .map(|project| (project.id, project.display_name()))
                    .collect::<Vec<_>>();
                let projectless = this.state.projects.iter().any(Project::is_projectless);
                let mut items = vec![MenuItem::Header(tr!("sidebar.projects").into()), {
                    let weak = weak.clone();
                    MenuItem::new(tr!("sidebar.all_projects"), move |_, cx| {
                        let _ = weak.update(cx, |this, cx| {
                            this.set_sidebar_project_filter(None, cx);
                        });
                    })
                    .icon("icons/folder.svg")
                    .selected(selected.is_none())
                }];
                if !project_options.is_empty() || projectless {
                    items.push(MenuItem::Separator);
                }
                for (project_id, project_name) in project_options {
                    let weak = weak.clone();
                    items.push(
                        MenuItem::new(project_name, move |_, cx| {
                            let _ = weak.update(cx, |this, cx| {
                                this.set_sidebar_project_filter(
                                    Some(SidebarProjectFilter::Project(project_id)),
                                    cx,
                                );
                            });
                        })
                        .icon("icons/folder.svg")
                        .selected(selected == Some(SidebarProjectFilter::Project(project_id))),
                    );
                }
                if projectless {
                    let weak = weak.clone();
                    items.push(
                        MenuItem::new(tr!("project.no_project_name"), move |_, cx| {
                            let _ = weak.update(cx, |this, cx| {
                                this.set_sidebar_project_filter(
                                    Some(SidebarProjectFilter::Projectless),
                                    cx,
                                );
                            });
                        })
                        .icon("icons/x.svg")
                        .selected(selected == Some(SidebarProjectFilter::Projectless)),
                    );
                }
                items
            },
        );
        div()
            .w_full()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(2.0))
            .child(picker)
    }

    fn render_sidebar_filters(&self, cx: &mut Context<Self>) -> Div {
        let theme = Theme::current(cx);
        let handle = self.menu_handle("sidebar-filters", cx);
        let weak = cx.entity().downgrade();
        let active_count = self.sidebar_active_filter_count();
        let trigger = div()
            .id("sidebar-filters")
            .w_full()
            .h(px(SIDEBAR_ACTION_ROW_HEIGHT))
            .px(px(4.0))
            .rounded(px(7.0))
            .flex()
            .items_center()
            .gap(px(10.0))
            .cursor_default()
            .focus_visible(|style| style.border_1().border_color(theme.accent))
            .hover(|element| element.bg(theme.sidebar_item_background))
            .when(handle.is_open(), |element| {
                element.bg(theme.sidebar_item_background)
            })
            .tooltip(Tooltip::text(tr_cow!("sidebar.filters")))
            .child(icon(
                "icons/sliders.svg",
                15.0,
                if active_count > 0 {
                    theme.accent
                } else {
                    theme.text_secondary
                },
            ))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_size(px(13.0))
                    .text_color(theme.text_secondary)
                    .child(tr!("sidebar.filters")),
            )
            .when(active_count > 0, |element| {
                element.child(
                    div()
                        .flex_none()
                        .w(px(16.0))
                        .h(px(16.0))
                        .rounded_full()
                        .bg(theme.accent.opacity(0.14))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(10.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.accent)
                        .child(SharedString::from(active_count.to_string())),
                )
            })
            .child(icon("icons/chevron-down.svg", 11.0, theme.text_ghost));
        let picker = dropdown_menu(
            trigger,
            "sidebar-filters-menu",
            &handle,
            MenuAlign::BelowRight,
            move |cx| {
                let Some(entity) = weak.upgrade() else {
                    return Vec::new();
                };
                let this = entity.read(cx);
                let selected_provider = this.sidebar_provider_filter;
                let selected_status = this.sidebar_status_filter;
                let mut items = Vec::new();
                if active_count > 0 {
                    let weak = weak.clone();
                    items.push(
                        MenuItem::new(tr!("sidebar.clear_filters"), move |_, cx| {
                            let _ = weak.update(cx, |this, cx| {
                                this.clear_sidebar_filters(cx);
                            });
                        })
                        .icon("icons/x.svg"),
                    );
                    items.push(MenuItem::Separator);
                }
                items.push(MenuItem::Header(tr!("sidebar.provider").into()));
                {
                    let weak = weak.clone();
                    items.push(
                        MenuItem::new(tr!("sidebar.all_providers"), move |_, cx| {
                            let _ = weak.update(cx, |this, cx| {
                                this.set_sidebar_provider_filter(None, cx);
                            });
                        })
                        .icon("icons/bot.svg")
                        .selected(selected_provider.is_none()),
                    );
                }
                for provider in ProviderKind::ALL {
                    if !this
                        .state
                        .sessions
                        .iter()
                        .any(|session| session.has_started() && session.provider == provider)
                    {
                        continue;
                    }
                    let weak = weak.clone();
                    items.push(
                        MenuItem::new(provider.display_name(), move |_, cx| {
                            let _ = weak.update(cx, |this, cx| {
                                this.set_sidebar_provider_filter(Some(provider), cx);
                            });
                        })
                        .icon(provider_icon(provider))
                        .selected(selected_provider == Some(provider)),
                    );
                }
                items.push(MenuItem::Separator);
                items.push(MenuItem::Header(tr!("sidebar.status").into()));
                for status in [
                    SidebarStatusFilter::All,
                    SidebarStatusFilter::Active,
                    SidebarStatusFilter::NeedsYou,
                    SidebarStatusFilter::Done,
                    SidebarStatusFilter::Failed,
                ] {
                    let weak = weak.clone();
                    items.push(
                        MenuItem::new(status.label(), move |_, cx| {
                            let _ = weak.update(cx, |this, cx| {
                                this.sidebar_status_filter = status;
                                cx.notify();
                            });
                        })
                        .icon(match status {
                            SidebarStatusFilter::All => "icons/gauge.svg",
                            SidebarStatusFilter::Active => "icons/loader-circle.svg",
                            SidebarStatusFilter::NeedsYou => "icons/alert.svg",
                            SidebarStatusFilter::Done => "icons/check.svg",
                            SidebarStatusFilter::Failed => "icons/x.svg",
                        })
                        .selected(selected_status == status),
                    );
                }
                items
            },
        );
        div().w_full().child(picker)
    }

    /// A compact removable pill for one active filter: provider mark + label +
    /// an X that clears just that filter. Activation follows the shared
    /// keyboard convention (`keyboard_activate`) so it works identically by
    /// click and by Enter/Space on every platform.
    fn sidebar_filter_chip(
        &self,
        id: &'static str,
        kind: SidebarFilterChipKind,
        icon_path: &'static str,
        icon_color: Hsla,
        label: SharedString,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = Theme::current(cx);
        let remove_id: SharedString = format!("{id}-remove").into();
        let remove = crate::ui::accessibility::focus_ring(
            div()
                .id(remove_id)
                .size(px(15.0))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_default()
                .hover(|element| element.bg(theme.overlay_strong))
                .child(icon("icons/x.svg", 10.0, theme.text_tertiary)),
            &theme,
        );
        let remove = crate::ui::accessibility::keyboard_activate(remove, cx, move |this, _, cx| {
            this.clear_sidebar_filter_chip(kind, cx);
        });
        div()
            .id(id)
            .h(px(22.0))
            .pl(px(7.0))
            .pr(px(4.0))
            .rounded_full()
            .bg(theme.overlay)
            .border_1()
            .border_color(theme.border)
            .flex()
            .items_center()
            .gap(px(4.0))
            .cursor_default()
            .child(icon(icon_path, 11.0, icon_color))
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.text_secondary)
                    .child(label),
            )
            .child(remove)
    }

    /// The removable chips for the active filters, rendered right under the
    /// Filters row so the current scope is visible at a glance and each one can
    /// be dropped with one click. Absent when nothing is filtered, so the chrome
    /// stays a single clean row in the common case.
    fn render_sidebar_filter_chips(&self, cx: &mut Context<Self>) -> Option<Div> {
        let provider = self.sidebar_provider_filter;
        let status = self.sidebar_status_filter;
        if provider.is_none() && status == SidebarStatusFilter::All {
            return None;
        }

        let theme = Theme::current(cx);
        let mut chips = div().flex().flex_wrap().gap(px(6.0)).pt(px(6.0));

        if let Some(provider) = provider {
            chips = chips.child(self.sidebar_filter_chip(
                "sidebar-chip-provider",
                SidebarFilterChipKind::Provider,
                provider_icon(provider),
                provider_color(&theme, provider),
                provider.display_name().into(),
                cx,
            ));
        }
        if status != SidebarStatusFilter::All {
            chips = chips.child(self.sidebar_filter_chip(
                "sidebar-chip-status",
                SidebarFilterChipKind::Status,
                match status {
                    SidebarStatusFilter::All => "icons/gauge.svg",
                    SidebarStatusFilter::Active => "icons/loader-circle.svg",
                    SidebarStatusFilter::NeedsYou => "icons/alert.svg",
                    SidebarStatusFilter::Done => "icons/check.svg",
                    SidebarStatusFilter::Failed => "icons/x.svg",
                },
                match status {
                    SidebarStatusFilter::All => theme.text_tertiary,
                    SidebarStatusFilter::Active => theme.accent,
                    SidebarStatusFilter::NeedsYou => theme.warning,
                    SidebarStatusFilter::Done => theme.success,
                    SidebarStatusFilter::Failed => theme.danger,
                },
                status.label().into(),
                cx,
            ));
        }

        chips = chips.child(
            crate::ui::accessibility::keyboard_activate(
                crate::ui::accessibility::focus_ring(
                    div()
                        .id("sidebar-clear-filters")
                        .h(px(22.0))
                        .px(px(7.0))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .cursor_default()
                        .text_size(px(11.0))
                        .text_color(theme.text_tertiary)
                        .hover(|element| {
                            element.bg(theme.overlay).text_color(theme.text_secondary)
                        })
                        .child(tr!("sidebar.clear_filters")),
                    &theme,
                ),
                cx,
                move |this, _, cx| this.clear_sidebar_filters(cx),
            ),
        );

        Some(chips)
    }

    fn set_sidebar_provider_filter(
        &mut self,
        provider_filter: Option<ProviderKind>,
        cx: &mut Context<Self>,
    ) {
        if self.sidebar_provider_filter == provider_filter {
            return;
        }
        self.sidebar_provider_filter = provider_filter;
        cx.notify();
    }

    fn clear_sidebar_filters(&mut self, cx: &mut Context<Self>) {
        self.sidebar_provider_filter = None;
        self.sidebar_status_filter = SidebarStatusFilter::All;
        cx.notify();
    }

    fn set_sidebar_project_filter(
        &mut self,
        project_filter: Option<SidebarProjectFilter>,
        cx: &mut Context<Self>,
    ) {
        if self.sidebar_project_filter == project_filter {
            return;
        }
        self.sidebar_project_filter = project_filter;
        cx.notify();
    }

    /// The sidebar's primary action. Sits directly under the project scope
    /// selector and is the only filled control in the chrome, so "start
    /// something new" reads at a glance before the quieter search/filter rows.
    fn render_sidebar_new_session(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let theme = Theme::current(cx);
        let element = crate::ui::accessibility::focus_ring(
            div()
                .id("sidebar-new-session")
                .w_full()
                .h(px(32.0))
                .mt(px(4.0))
                .mb(px(2.0))
                .px(px(9.0))
                .rounded(px(8.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .cursor_default()
                .bg(theme.inverse)
                .text_color(theme.on_inverse)
                .text_size(px(12.5))
                .font_weight(FontWeight::SEMIBOLD)
                .hover(|element| element.opacity(0.92))
                .active(|element| element.opacity(0.8))
                .child(icon("icons/compose.svg", 14.0, theme.on_inverse))
                .child(tr!("menu.new_task"))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.new_session_action(&NewSession, window, cx);
                })),
            &theme,
        );
        crate::ui::accessibility::keyboard_activate(element, cx, |this, window, cx| {
            this.new_session_action(&NewSession, window, cx);
        })
    }

    fn render_sidebar_search(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        crate::ui::accessibility::keyboard_activate(
            self.render_sidebar_action_row(
                "sidebar-search",
                "icons/search.svg",
                tr!("sidebar.search"),
                cx,
            )
            .on_click(cx.listener(|this, _, window, cx| {
                this.toggle_command_palette_action(&ToggleCommandPalette, window, cx);
            })),
            cx,
            |this, window, cx| {
                this.toggle_command_palette_action(&ToggleCommandPalette, window, cx);
            },
        )
    }

    fn start_available_update(&mut self, cx: &mut Context<Self>) {
        if self.updater_status != crate::updater::UpdateStatus::Available {
            return;
        }
        let started = cx
            .try_global::<crate::updater::UpdaterState>()
            .and_then(|state| state.0.as_ref())
            .is_some_and(|updater| updater.install_available_update());
        if started {
            self.updater_status = crate::updater::UpdateStatus::Updating;
            self.reset_updater_button_animation();
            cx.notify();
        }
    }

    fn render_updater_button(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let status = self.updater_status;
        if status == crate::updater::UpdateStatus::Idle {
            return None;
        }

        let theme = Theme::current(cx);
        let foreground = rgb(0xFFFFFF).into();
        let available = status == crate::updater::UpdateStatus::Available;
        let button = div()
            .id("sidebar-update")
            .track_focus(&self.updater_button_focus)
            .when(available, |button| button.tab_index(0))
            .w(px(UPDATER_BUTTON_COLLAPSED_WIDTH))
            .h(px(20.0))
            .flex_none()
            .overflow_hidden()
            .rounded_full()
            .relative()
            .cursor_default()
            .bg(theme.gauge)
            .text_color(foreground)
            .text_size(px(11.0))
            .font_weight(FontWeight::MEDIUM)
            .when(available, |button| {
                button
                    .hover(|style| style.opacity(0.92))
                    .focus_visible(|style| style.border_1().border_color(rgb(0xFFFFFF)))
                    .active(|style| style.opacity(0.8))
                    .on_hover(cx.listener(|this, hovering: &bool, _, cx| {
                        this.set_updater_button_hovered(*hovering, cx);
                    }))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.start_available_update(cx);
                    }))
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                            this.start_available_update(cx);
                            cx.stop_propagation();
                        }
                    }))
            });

        if !available {
            let indicator = icon("icons/loader-circle.svg", 14.0, foreground)
                .with_animation(
                    "sidebar-updater-spinner",
                    Animation::new(Duration::from_millis(900))
                        .repeat()
                        .with_easing(gpui::linear),
                    |icon, delta| {
                        icon.with_transformation(gpui::Transformation::rotate(gpui::percentage(
                            delta,
                        )))
                    },
                )
                .into_any_element();
            return Some(
                button
                    .tooltip(Tooltip::text(
                        if status == crate::updater::UpdateStatus::Checking {
                            tr!("updater.checking")
                        } else {
                            tr!("updater.updating")
                        },
                    ))
                    .child(
                        div()
                            .size_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(indicator),
                    )
                    .into_any_element(),
            );
        }

        let label: SharedString = tr_cow!("updater.update").into();
        let animation_generation = self.updater_button_animation_generation;
        if animation_generation == 0 {
            return Some(
                button
                    .child(updater_button_available_content(foreground, label, 0.0))
                    .into_any_element(),
            );
        }

        let from_width = self.updater_button_animation_from_width;
        let from_reveal = self.updater_button_animation_from_reveal;
        let target_width = if self.updater_button_expanded() {
            UPDATER_BUTTON_EXPANDED_WIDTH
        } else {
            UPDATER_BUTTON_COLLAPSED_WIDTH
        };
        let target_reveal = if self.updater_button_expanded() {
            1.0
        } else {
            0.0
        };
        let current_width = self.updater_button_width.clone();
        let current_reveal = self.updater_button_label_reveal.clone();

        Some(
            button
                .with_animation(
                    SharedString::from(format!("sidebar-updater-expand-{animation_generation}")),
                    Animation::new(Duration::from_millis(150)).with_easing(ease_out_quint()),
                    move |button, delta| {
                        let width = from_width + (target_width - from_width) * delta;
                        let reveal = from_reveal + (target_reveal - from_reveal) * delta;
                        current_width.set(width);
                        current_reveal.set(reveal);
                        button.w(px(width)).child(updater_button_available_content(
                            foreground,
                            label.clone(),
                            reveal,
                        ))
                    },
                )
                .into_any_element(),
        )
    }

    fn render_sidebar_footer(&self, cx: &mut Context<Self>) -> Div {
        let theme = Theme::current(cx);
        div()
            .flex_none()
            .h(px(40.0))
            .px(px(10.0))
            .flex()
            .items_center()
            .child(
                div()
                    .id("open-settings")
                    .tab_index(0)
                    .focus_visible(|style| style.border_1().border_color(theme.accent))
                    .w(px(26.0))
                    .h(px(26.0))
                    .flex_none()
                    .rounded(px(6.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_default()
                    .hover(|element| element.bg(theme.overlay))
                    .active(|element| element.bg(theme.overlay_strong))
                    .tooltip(Tooltip::text(tr_cow!("common.settings")))
                    .child(icon("icons/settings.svg", 14.0, theme.text_tertiary))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_settings_action(&OpenSettings, window, cx);
                    })),
            )
            .child(div().flex_1())
            .when_some(self.render_updater_button(cx), |footer, button| {
                footer.child(button)
            })
    }

    pub(super) fn render_sidebar(&self, width: f32, cx: &mut Context<Self>) -> Div {
        let theme = Theme::current(cx);
        let is_resizing = self
            .panel_resize_drag
            .is_some_and(|drag| drag.target == PanelResizeTarget::Sidebar);

        // Building the row snapshot is cheap (a few bytes per session); the
        // heavy element construction happens only for rows the list can see.
        let rows = Rc::new(self.sidebar_rows());
        self.sync_sidebar_rows(&rows);
        let history_scrolled =
            self.sidebar_list_state.scroll_px_offset_for_scrollbar().y < px(-0.5);
        let entity = cx.entity().downgrade();

        div()
            .w(px(width))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .bg(if is_resizing {
                theme.sidebar_drag_background
            } else {
                theme.sidebar
            })
            .child(self.render_sidebar_titlebar(cx))
            .child(self.render_sidebar_chrome(cx))
            .child(
                div()
                    .id("sidebar-scroll")
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(
                        div().px(px(10.0)).size_full().child(
                            list(
                                self.sidebar_list_state.clone(),
                                move |index, _window, cx| {
                                    entity
                                        .upgrade()
                                        .map(|entity| {
                                            entity.update(cx, |this, cx| {
                                                this.sidebar_row(index, &rows, cx)
                                            })
                                        })
                                        .unwrap_or_else(|| div().into_any_element())
                                },
                            )
                            .size_full(),
                        ),
                    )
                    .child(scrollbar::vertical(
                        &self.sidebar_list_state,
                        &self.sidebar_scrollbar,
                    ))
                    .when(history_scrolled, |scroll| {
                        scroll.child(
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .w_full()
                                .h(px(1.0))
                                .bg(theme.border),
                        )
                    }),
            )
            .child(self.render_sidebar_footer(cx))
    }

    /// Snapshot the session history as a flat list of lightweight rows, newest
    /// first, grouped by calendar period like the previous eager render.
    fn sidebar_rows(&self) -> Vec<SidebarRow> {
        let project_filter = self.resolved_sidebar_project_filter();
        let provider_filter = self.sidebar_provider_filter;
        let status_filter = self.sidebar_status_filter;
        let projectless_ids = collect_projectless_project_ids(&self.state.projects);
        let mut sorted_sessions = self
            .state
            .sessions
            .iter()
            .filter(|session| {
                sidebar_session_matches_project_filter(
                    session,
                    project_filter,
                    provider_filter,
                    &projectless_ids,
                ) && status_filter.matches(session.status)
            })
            .collect::<Vec<_>>();
        sorted_sessions
            .sort_by_key(|session| std::cmp::Reverse(sidebar_session_timestamp(session)));

        let mut groups: Vec<(Uuid, Vec<Uuid>)> = Vec::new();
        for session in sorted_sessions {
            if let Some((_, sessions)) = groups
                .iter_mut()
                .find(|(project_id, _)| *project_id == session.project_id)
            {
                sessions.push(session.id);
            } else {
                groups.push((session.project_id, vec![session.id]));
            }
        }
        let mut rows = Vec::new();
        for (project_id, sessions) in groups {
            rows.push(SidebarRow::ProjectHeader(project_id, sessions.len()));
            rows.extend(sessions.into_iter().map(SidebarRow::Session));
            rows.push(SidebarRow::GroupSpacer);
        }
        if rows.is_empty() && self.state.sessions.iter().any(AgentSession::has_started) {
            rows.push(SidebarRow::EmptyFilter);
        }
        rows
    }

    fn render_sidebar_empty_filter(&self, cx: &mut Context<Self>) -> Div {
        let theme = Theme::current(cx);
        div()
            .w_full()
            .px(px(8.0))
            .pt(px(10.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .text_size(px(12.5))
                    .text_color(theme.text_tertiary)
                    .child(tr_cow!("sidebar.no_matching_tasks")),
            )
            .child(
                div()
                    .id("sidebar-show-all-projects")
                    .tab_index(0)
                    .w_full()
                    .h(px(SIDEBAR_ACTION_ROW_HEIGHT))
                    .px(px(4.0))
                    .rounded(px(7.0))
                    .flex()
                    .items_center()
                    .cursor_default()
                    .text_size(px(13.0))
                    .text_color(theme.text_secondary)
                    .focus_visible(|style| style.border_1().border_color(theme.accent))
                    .hover(|element| element.bg(theme.sidebar_item_background))
                    .child(tr_cow!("sidebar.clear_filters"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.set_sidebar_project_filter(None, cx);
                        this.set_sidebar_provider_filter(None, cx);
                        this.sidebar_status_filter = SidebarStatusFilter::All;
                        cx.notify();
                    }))
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                            this.set_sidebar_project_filter(None, cx);
                            this.set_sidebar_provider_filter(None, cx);
                            this.sidebar_status_filter = SidebarStatusFilter::All;
                            cx.notify();
                            cx.stop_propagation();
                        }
                    })),
            )
    }

    /// Keep the virtualized list in sync with the current row snapshot.
    /// Rows are cheap values, so only the minimal changed suffix is spliced,
    /// preserving scroll position and measured heights across unrelated churn
    /// (e.g. the active session's `updated_at` bumping on every stream tick).
    fn sync_sidebar_rows(&self, rows: &[SidebarRow]) {
        let mut cached = self.sidebar_row_cache.borrow_mut();
        if cached.as_slice() == rows {
            return;
        }
        let prefix = cached
            .iter()
            .zip(rows.iter())
            .take_while(|(a, b)| a == b)
            .count();
        let old_count = cached.len();
        *cached = rows.to_vec();
        if old_count == 0 {
            self.sidebar_list_state
                .reset_with_uniform_height(rows.len(), px(SIDEBAR_SESSION_ROW_HEIGHT));
        } else {
            self.sidebar_list_state
                .splice(prefix..old_count, rows.len() - prefix);
            // Newly inserted rows have no measured height yet; give them the
            // uniform hint so the scrollbar keeps a correct total height.
            self.sidebar_list_state
                .clone()
                .with_uniform_item_height(px(SIDEBAR_SESSION_ROW_HEIGHT));
        }
    }

    fn sidebar_row(&self, index: usize, rows: &[SidebarRow], cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = rows.get(index) else {
            return div().into_any_element();
        };
        match *row {
            SidebarRow::ProjectHeader(project_id, task_count) => self
                .render_sidebar_project_header(project_id, task_count, cx)
                .into_any_element(),
            SidebarRow::Session(session_id) => self
                .render_sidebar_session_item(session_id, cx)
                .into_any_element(),
            SidebarRow::GroupSpacer => div().w_full().h(px(10.0)).into_any_element(),
            SidebarRow::EmptyFilter => self.render_sidebar_empty_filter(cx).into_any_element(),
        }
    }

    fn render_sidebar_project_header(
        &self,
        project_id: Uuid,
        task_count: usize,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = Theme::current(cx);
        let project_name = self
            .state
            .projects
            .iter()
            .find(|project| project.id == project_id)
            .map(Project::display_name)
            .unwrap_or_else(|| tr!("sidebar.unknown_project"));
        let count_label = if task_count == 1 {
            tr!("sidebar.task_count_one", count = task_count)
        } else {
            tr!("sidebar.task_count_many", count = task_count)
        };
        session_group_header(&theme).w_full().child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .min_w_0()
                .flex_1()
                .child(icon("icons/folder.svg", 12.0, theme.text_tertiary))
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .truncate()
                        .text_color(theme.text_tertiary)
                        .child(project_name),
                )
                .child(
                    div()
                        .flex_none()
                        .text_color(theme.text_ghost)
                        .child(SharedString::from(count_label)),
                ),
        )
    }

    fn begin_session_rename(
        &mut self,
        session_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(title) = self
            .state
            .sessions
            .iter()
            .find(|session| session.id == session_id)
            .map(localized_session_title)
        else {
            return;
        };

        self.session_rename = Some(session_id);
        self.session_rename_input.update(cx, |input, cx| {
            input.set_content(title, cx);
            input.select_all_text(cx);
        });
        let focus = self.session_rename_input.read(cx).focus();
        window.on_next_frame(move |window, cx| window.focus(&focus, cx));
        cx.notify();
    }

    pub(super) fn commit_session_rename(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.session_rename.take() else {
            return;
        };
        let title = self
            .session_rename_input
            .read(cx)
            .content()
            .trim()
            .to_owned();
        let should_update = !title.is_empty()
            && self
                .state
                .sessions
                .iter()
                .find(|session| session.id == session_id)
                .is_some_and(|session| session.title != title);
        if should_update
            && self
                .state
                .session_mut(session_id)
                .is_some_and(|session| session.set_title(&title))
        {
            self.save();
        }
        cx.notify();
    }

    fn cancel_session_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.session_rename.take().is_none() {
            return;
        }
        let focus = self.composer_focus(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    fn render_sidebar_session_item(&self, session_id: Uuid, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::current(cx);
        let Some(session) = self
            .state
            .sessions
            .iter()
            .find(|session| session.id == session_id)
        else {
            return div().into_any_element();
        };
        let selected = self.state.selected_session == Some(session_id);
        let working = matches!(
            session.status,
            SessionStatus::Connecting | SessionStatus::Working
        );
        let status_label = match session.status {
            SessionStatus::Connecting | SessionStatus::Working => Some(tr!("sidebar.active")),
            SessionStatus::Waiting => Some(tr!("sidebar.needs_you")),
            SessionStatus::Failed => Some(tr!("sidebar.failed")),
            SessionStatus::Idle => None,
        };
        let time_label = session_time_label(session, unix_time());
        let time_color = if session.is_busy() {
            theme.text_tertiary
        } else {
            theme.text_ghost
        };
        let rename_input =
            (self.session_rename == Some(session_id)).then(|| self.session_rename_input.clone());
        let renaming = rename_input.is_some();
        let title = if let Some(rename_input) = rename_input {
            div()
                .id(SharedString::from(format!(
                    "session-rename-field-{session_id}"
                )))
                .key_context(SESSION_RENAME_PARENT_CONTEXT)
                .on_action(cx.listener(|this, _: &CancelSessionRename, window, cx| {
                    this.cancel_session_rename(window, cx);
                }))
                .h(px(18.0))
                .flex_1()
                .min_w_0()
                .px(px(4.0))
                .rounded(px(4.0))
                .border_1()
                .border_color(theme.accent)
                .bg(theme.inset)
                .flex()
                .items_center()
                .text_size(px(13.5))
                .text_color(theme.text)
                .child(rename_input)
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_w_0()
                .whitespace_normal()
                .line_clamp(1)
                .text_overflow(gpui::TextOverflow::Truncate("...".into()))
                .text_size(px(13.5))
                .text_color(theme.text)
                .when(selected, |element| {
                    element.font_weight(FontWeight::MEDIUM)
                })
                .child(SharedString::from(localized_session_title(session)))
                .into_any_element()
        };
        let waku = cx.entity().downgrade();
        let menu = self.menu_handle(format!("session-{session_id}"), cx);
        let row_focus = menu.trigger_focus_handle().clone();
        let keyboard_menu = menu.clone();
        let row = div()
            .id(SharedString::from(format!("session-{}", session.id)))
            .w_full()
            .min_w_0()
            .relative()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .px(px(11.0))
            .py(px(7.0))
            .rounded(px(7.0))
            .cursor_default()
            .when(selected, |element| {
                element
                    .bg(theme.overlay_strong)
                    .child(
                        div()
                            .absolute()
                            .left(px(0.0))
                            .top(px(9.0))
                            .bottom(px(9.0))
                            .w(px(3.0))
                            .rounded_full()
                            .bg(theme.accent),
                    )
            })
            .when(!selected, |element| {
                element.hover(|element| element.bg(theme.overlay))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .overflow_hidden()
                    .line_height(px(18.0))
                    .child(title),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.0))
                    .text_size(px(11.5))
                    .line_height(px(15.0))
                    .when(working, |element| {
                        element.child(
                            icon(
                                "icons/loader-circle.svg",
                                12.0,
                                status_color(&theme, session.status),
                            )
                            .with_animation(
                                SharedString::from(format!("session-spinner-{session_id}")),
                                Animation::new(Duration::from_millis(900))
                                    .repeat()
                                    .with_easing(gpui::linear),
                                |icon, delta| {
                                    icon.with_transformation(gpui::Transformation::rotate(
                                        gpui::percentage(delta),
                                    ))
                                },
                            ),
                        )
                    })
                    .when(session.status == SessionStatus::Waiting, |element| {
                        element.child(icon(
                            "icons/alert.svg",
                            12.0,
                            status_color(&theme, session.status),
                        ))
                    })
                    .when(session.status == SessionStatus::Failed, |element| {
                        element.child(icon(
                            "icons/x.svg",
                            12.0,
                            status_color(&theme, session.status),
                        ))
                    })
                    .when(
                        session.has_started() && session.status == SessionStatus::Idle,
                        |element| element.child(icon("icons/check.svg", 12.0, theme.success)),
                    )
                    .when_some(status_label, |element, label| {
                        element.child(
                            div()
                                .flex_none()
                                .text_color(status_color(&theme, session.status))
                                .child(SharedString::from(label)),
                        )
                    })
                    .child(icon(
                        provider_icon(session.provider),
                        11.0,
                        provider_color(&theme, session.provider),
                    ))
                    .child(
                        div()
                            .flex_none()
                            .text_color(theme.text_tertiary)
                            .child(session.provider.short_name()),
                    )
                    .child(
                        div().flex_1().min_w(px(0.0)),
                    )
                    .when_some(time_label, |element, label| {
                        element.child(
                            div()
                                .flex_none()
                                .text_color(time_color)
                                .child(SharedString::from(label)),
                        )
                    }),
            )
            .when(!renaming, |element| {
                element
                    .track_focus(&row_focus)
                    .tab_index(0)
                    .focus_visible(|style| style.border_1().border_color(theme.accent))
                    .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                        let key = event.keystroke.key.as_str();
                        if matches!(key, "enter" | "space") {
                            this.select_session(session_id, cx);
                            cx.stop_propagation();
                        } else if key == "f10" && event.keystroke.modifiers.shift {
                            keyboard_menu.open_context_menu(window, cx);
                            cx.stop_propagation();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_session(session_id, cx);
                    }))
            });
        let row = if renaming {
            div()
                .w_full()
                .child(row)
                .on_mouse_down_out(cx.listener(move |this, _, _, cx| {
                    if this.session_rename == Some(session_id) {
                        this.commit_session_rename(cx);
                    }
                }))
                .into_any_element()
        } else {
            context_menu(
                div().w_full().child(row),
                SharedString::from(format!("session-menu-{session_id}")),
                &menu,
                move |_| {
                    let rename_waku = waku.clone();
                    let remove_waku = waku.clone();
                    vec![
                        MenuItem::new(tr!("common.rename"), move |window, cx| {
                            let _ = rename_waku.update(cx, |waku, cx| {
                                waku.begin_session_rename(session_id, window, cx);
                            });
                        }),
                        MenuItem::Separator,
                        MenuItem::new(tr!("common.remove"), move |_, cx| {
                            let _ = remove_waku
                                .update(cx, |waku, cx| waku.remove_session(session_id, cx));
                        }),
                    ]
                },
            )
        };

        div()
            .w_full()
            .pb(px(SIDEBAR_SESSION_ROW_GAP))
            .child(row)
            .into_any_element()
    }

    // ── Header ─────────────────────────────────────────────────────────────

    pub(super) fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::current(cx);
        let session = self.selected_session();
        let title = session
            .map(localized_session_title)
            .unwrap_or_else(|| tr!("session.new_task"));
        let agent_preset_label = session
            .filter(|session| session.provider == ProviderKind::DeepSeek && session.has_started())
            .and_then(|session| self.agent_preset_label_for_session(session));
        div()
            .id("window-header")
            .h(px(48.0))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.0))
            .pl(if self.sidebar_visible {
                px(14.0)
            } else {
                px(0.0)
            })
            .pr(px(
                if !cfg!(target_os = "macos") && !self.right_panel_visible {
                    WINDOW_CONTROLS_WIDTH + 14.0
                } else {
                    14.0
                },
            ))
            .when(!self.sidebar_visible, |element| {
                element
                    .child(
                        self.window_drag_region(
                            div()
                                .id("header-traffic-light-drag-region")
                                .w(px((TRAFFIC_LIGHT_CLEARANCE - 8.0).max(0.0)))
                                .h_full()
                                .flex_none(),
                            cx,
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(self.render_sidebar_toggle(cx))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(2.0))
                                    .child(self.render_history_button(
                                        "navigate-back",
                                        "icons/arrow-left.svg",
                                        !self.session_navigation.back.is_empty(),
                                        true,
                                        cx,
                                    ))
                                    .child(self.render_history_button(
                                        "navigate-forward",
                                        "icons/arrow-right.svg",
                                        !self.session_navigation.forward.is_empty(),
                                        false,
                                        cx,
                                    )),
                            ),
                    )
            })
            .child(
                self.window_drag_region(
                    div()
                        .id("header-title-drag-region")
                        .h_full()
                        .min_w_0()
                        .flex_shrink(1.0)
                        .flex()
                        .items_center()
                        .gap(px(7.0))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.text)
                                .child(SharedString::from(title)),
                        )
                        .children(agent_preset_label.map(|label| {
                            div()
                                .h(px(22.0))
                                .max_w(px(180.0))
                                .px(px(6.0))
                                .rounded(px(6.0))
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap(px(4.0))
                                .bg(theme.overlay)
                                .text_size(px(11.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.text_secondary)
                                .child(icon("icons/bot.svg", 10.5, theme.text_tertiary))
                                .child(div().min_w_0().truncate().child(SharedString::from(label)))
                        })),
                    cx,
                ),
            )
            .child(
                self.window_drag_region(
                    div().id("header-center-drag-region").h_full().flex_1(),
                    cx,
                ),
            )
            .child(self.render_background_work_summary(cx))
            .when(!self.right_panel_visible, |element| {
                element
                    .when(self.fps_counter_visible, |element| {
                        element.child(self.render_fps_counter(cx))
                    })
                    .child(self.render_right_panel_toggle(cx))
            })
    }

    // ── Empty states ───────────────────────────────────────────────────────

    pub(super) fn render_empty_state(&self, cx: &mut Context<Self>) -> Div {
        let theme = Theme::current(cx);
        if self.selected_project().is_none() {
            return div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .px_8()
                .pb(px(46.0))
                .child(icon("icons/sparkle.svg", 24.0, theme.accent))
                .child(
                    div()
                        .mt(px(16.0))
                        .text_size(px(20.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text)
                        .child(tr_cow!("onboarding.open_project_to_begin")),
                )
                .child(
                    div()
                        .mt(px(8.0))
                        .max_w(px(380.0))
                        .text_center()
                        .text_size(px(12.5))
                        .line_height(px(19.0))
                        .text_color(theme.text_tertiary)
                        .child(tr_cow!("onboarding.description")),
                )
                .child(
                    div()
                        .mt(px(20.0))
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(8.0))
                        .tab_index(0)
                        .tab_group()
                        .tab_stop(false)
                        .child(
                            div()
                                .id("onboarding-add-project")
                                .track_focus(&self.onboarding_add_project_focus)
                                .tab_index(0)
                                .focus_visible(|style| style.border_1().border_color(theme.accent))
                                .h(px(32.0))
                                .px(px(14.0))
                                .rounded_full()
                                .flex()
                                .items_center()
                                .cursor_default()
                                .bg(theme.inverse)
                                .text_color(theme.on_inverse)
                                .text_size(px(12.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .hover(|element| element.opacity(0.9))
                                .active(|element| element.opacity(0.8))
                                .child(tr_cow!("onboarding.open_project_folder"))
                                .on_click(cx.listener(|this, _, _, cx| this.add_project(cx)))
                                .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                        this.add_project(cx);
                                        cx.stop_propagation();
                                    }
                                })),
                        )
                        .child(
                            div()
                                .id("onboarding-projectless")
                                .track_focus(&self.onboarding_projectless_focus)
                                .tab_index(1)
                                .focus_visible(|style| style.border_1().border_color(theme.accent))
                                .h(px(30.0))
                                .px(px(12.0))
                                .rounded_full()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .cursor_default()
                                .text_color(theme.text_secondary)
                                .text_size(px(12.0))
                                .hover(|element| element.bg(theme.overlay))
                                .active(|element| element.bg(theme.overlay_strong))
                                .child(icon("icons/x.svg", 11.0, theme.text_tertiary))
                                .child(tr_cow!("project.no_project"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.create_projectless_session(cx);
                                }))
                                .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                        this.create_projectless_session(cx);
                                        cx.stop_propagation();
                                    }
                                })),
                        ),
                );
        }
        let selected_project_id = self.state.selected_project;
        let projectless_selected = self.selected_project().is_some_and(Project::is_projectless);
        let project_name = self
            .selected_project()
            .map(|project| {
                if project.is_projectless() {
                    tr!("project.without_a_project")
                } else {
                    project.display_name()
                }
            })
            .unwrap_or_else(|| tr!("project.your_project"));
        let project_options = self
            .state
            .projects
            .iter()
            .filter(|project| !project.is_projectless())
            .filter(|project| Some(project.id) == selected_project_id)
            .chain(
                self.state
                    .projects
                    .iter()
                    .filter(|project| !project.is_projectless())
                    .filter(|project| Some(project.id) != selected_project_id),
            )
            .map(|project| (project.id, project.display_name()))
            .collect::<Vec<_>>();
        let weak = cx.entity().downgrade();
        let handle = self.menu_handle("empty-state-project", cx);
        let project_selector = dropdown_menu(
            ProjectNameSelector::new("empty-state-project", project_name)
                .selected(handle.is_open()),
            "empty-state-project-menu",
            &handle,
            MenuAlign::BelowLeft,
            move |_| {
                let mut items = project_options
                    .clone()
                    .into_iter()
                    .map(|(project_id, project_name)| {
                        let weak = weak.clone();
                        MenuItem::new(project_name, move |_, cx| {
                            if Some(project_id) == selected_project_id {
                                return;
                            }
                            let _ = weak.update(cx, |this, cx| this.select_project(project_id, cx));
                        })
                        .selected(Some(project_id) == selected_project_id)
                    })
                    .collect::<Vec<_>>();
                if !items.is_empty() {
                    items.push(MenuItem::Separator);
                }
                let add_project_weak = weak.clone();
                items.push(
                    MenuItem::new(tr!("project.new_project"), move |_, cx| {
                        let _ = add_project_weak.update(cx, |this, cx| this.add_project(cx));
                    })
                    .icon("icons/folder-new.svg"),
                );
                let projectless_weak = weak.clone();
                items.push(
                    MenuItem::new(tr!("project.no_project"), move |_, cx| {
                        let _ = projectless_weak.update(cx, |this, cx| {
                            if !this.selected_project().is_some_and(Project::is_projectless) {
                                this.create_projectless_session(cx);
                            }
                        });
                    })
                    .icon("icons/x.svg")
                    .selected(projectless_selected),
                );
                items
            },
        );
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .px_8()
            .pb(px(52.0))
            .child(icon("icons/sparkle.svg", 20.0, theme.accent))
            .child(
                div()
                    .mt(px(14.0))
                    .flex()
                    .items_baseline()
                    .text_size(px(20.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text)
                    .when(projectless_selected, |element| {
                        element.child(tr_cow!("onboarding.what_should_we_build"))
                    })
                    .when(!projectless_selected, |element| {
                        element
                            .child(tr_cow!("onboarding.what_should_we_build_in"))
                            .child(project_selector)
                            .child(tr_cow!("onboarding.question_mark"))
                    }),
            )
    }
}

fn localized_session_title(session: &AgentSession) -> String {
    let title = session.display_title();
    if title == AgentSession::DEFAULT_TITLE {
        tr!("session.new_task")
    } else {
        title.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidebar_recency_uses_last_reply_with_creation_fallback() {
        let project_id = Uuid::new_v4();
        let mut renamed_old_session = AgentSession::new(project_id, ProviderKind::Codex);
        renamed_old_session.created_at = 10;
        renamed_old_session.last_reply_at = Some(20);
        renamed_old_session.updated_at = 1_000;

        let mut newer_unanswered_session = AgentSession::new(project_id, ProviderKind::Codex);
        newer_unanswered_session.created_at = 30;
        newer_unanswered_session.last_reply_at = None;
        newer_unanswered_session.updated_at = 30;

        assert_eq!(sidebar_session_timestamp(&renamed_old_session), 20);
        assert_eq!(sidebar_session_timestamp(&newer_unanswered_session), 30);

        let mut sessions = [&renamed_old_session, &newer_unanswered_session];
        sessions.sort_by_key(|session| std::cmp::Reverse(sidebar_session_timestamp(session)));
        assert_eq!(sessions[0].id, newer_unanswered_session.id);
    }

    #[test]
    fn project_filter_keeps_matching_started_sessions() {
        let project_a = Uuid::from_u128(1);
        let project_b = Uuid::from_u128(2);
        let mut session = AgentSession::new(project_a, ProviderKind::Codex);
        session.messages.push(Message::new(MessageRole::User, "hi"));
        assert!(sidebar_session_matches_project_filter(
            &session,
            None,
            None,
            &HashSet::new()
        ));
        assert!(sidebar_session_matches_project_filter(
            &session,
            Some(SidebarProjectFilter::Project(project_a)),
            None,
            &HashSet::new()
        ));
        assert!(!sidebar_session_matches_project_filter(
            &session,
            Some(SidebarProjectFilter::Project(project_b)),
            None,
            &HashSet::new()
        ));
    }

    #[test]
    fn provider_filter_keeps_only_matching_started_sessions() {
        let project_id = Uuid::from_u128(1);
        let mut codex = AgentSession::new(project_id, ProviderKind::Codex);
        codex
            .messages
            .push(Message::new(MessageRole::User, "codex"));
        let mut claude = AgentSession::new(project_id, ProviderKind::Claude);
        claude
            .messages
            .push(Message::new(MessageRole::User, "claude"));
        assert!(sidebar_session_matches_project_filter(
            &codex,
            None,
            Some(ProviderKind::Codex),
            &HashSet::new()
        ));
        assert!(!sidebar_session_matches_project_filter(
            &claude,
            None,
            Some(ProviderKind::Codex),
            &HashSet::new()
        ));
    }

    #[test]
    fn status_filter_matches_orca_lifecycle_groups() {
        assert!(SidebarStatusFilter::Active.matches(SessionStatus::Working));
        assert!(SidebarStatusFilter::Active.matches(SessionStatus::Connecting));
        assert!(SidebarStatusFilter::NeedsYou.matches(SessionStatus::Waiting));
        assert!(SidebarStatusFilter::Done.matches(SessionStatus::Idle));
        assert!(SidebarStatusFilter::Failed.matches(SessionStatus::Failed));
        assert!(!SidebarStatusFilter::Done.matches(SessionStatus::Working));
    }

    #[test]
    fn unstarted_sessions_stay_out_of_the_sidebar() {
        let project_id = Uuid::from_u128(1);
        let session = AgentSession::new(project_id, ProviderKind::Codex);
        assert!(!sidebar_session_matches_project_filter(
            &session,
            None,
            None,
            &HashSet::new()
        ));
        assert!(!sidebar_session_matches_project_filter(
            &session,
            Some(SidebarProjectFilter::Project(project_id)),
            None,
            &HashSet::new()
        ));
    }

    #[test]
    fn projectless_filter_matches_all_projectless_projects() {
        let projectless_a = Uuid::from_u128(1);
        let projectless_b = Uuid::from_u128(2);
        let ordinary = Uuid::from_u128(3);
        let mut session_a = AgentSession::new(projectless_a, ProviderKind::Codex);
        session_a
            .messages
            .push(Message::new(MessageRole::User, "a"));
        let mut session_b = AgentSession::new(projectless_b, ProviderKind::Codex);
        session_b
            .messages
            .push(Message::new(MessageRole::User, "b"));
        let mut ordinary_session = AgentSession::new(ordinary, ProviderKind::Codex);
        ordinary_session
            .messages
            .push(Message::new(MessageRole::User, "ordinary"));
        let projects = vec![
            Project::from_path(dirs::home_dir().unwrap().join(".waku/2026-08-12/a")),
            Project::from_path(dirs::home_dir().unwrap().join(".waku/2026-08-12/b")),
            Project::from_path(std::path::PathBuf::from("D:/project")),
        ];
        session_a.project_id = projects[0].id;
        session_b.project_id = projects[1].id;
        ordinary_session.project_id = projects[2].id;
        let projectless_ids = collect_projectless_project_ids(&projects);

        assert!(sidebar_session_matches_project_filter(
            &session_a,
            Some(SidebarProjectFilter::Projectless),
            None,
            &projectless_ids,
        ));
        assert!(sidebar_session_matches_project_filter(
            &session_b,
            Some(SidebarProjectFilter::Projectless),
            None,
            &projectless_ids,
        ));
        assert!(!sidebar_session_matches_project_filter(
            &ordinary_session,
            Some(SidebarProjectFilter::Projectless),
            None,
            &projectless_ids,
        ));
    }
}
