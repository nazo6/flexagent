//! Client presence tracking for Web Push targeting.
//!
//! The hub learns which browser profile / installed PWA is currently in use from the
//! Client WS (`ClientWsMessage::Presence`). Presence is process-local and never
//! persisted: after a hub restart the registry is empty and the notification fallback
//! policy applies.
//!
//! A client is **active** when at least one of its tabs reported `visible && focused`
//! recently (see [`PRESENCE_TTL`]). The **recent** window is larger and includes tabs
//! that were active and were then hidden or blurred. All timestamps are server-side
//! receive times, so a client clock cannot influence targeting.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use fxg_protocol::{ClientId, SessionId, TabId};

/// How long a tab is considered active after its last presence report.
pub const PRESENCE_TTL: Duration = Duration::from_secs(90);

/// Drop tab entries that have not reported for this long.
///
/// Must stay above the longest recent window used for targeting
/// (`push::NotificationTargeting::recent_window`).
const PRESENCE_RETENTION: Duration = Duration::from_secs(60 * 60);

/// Presence report of one tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientPresence {
    /// Browser profile / installed PWA ID
    pub client_id: ClientId,
    /// Connection (tab) ID
    pub tab_id: TabId,
    /// Whether the document is visible
    pub visible: bool,
    /// Whether the window has focus
    pub focused: bool,
    /// Session currently open in this tab
    pub focused_session_id: Option<SessionId>,
}

#[derive(Debug, Clone)]
struct TabState {
    visible: bool,
    focused: bool,
    focused_session_id: Option<SessionId>,
    last_seen: Instant,
    /// Last time this tab was `visible && focused`.
    last_active: Option<Instant>,
}

/// Process-local presence of connected client tabs.
#[derive(Debug, Default)]
pub(crate) struct PresenceRegistry {
    tabs: std::sync::Mutex<HashMap<(ClientId, TabId), TabState>>,
}

impl PresenceRegistry {
    /// Records a presence report using the current time.
    pub(crate) fn record(&self, presence: ClientPresence) {
        self.record_at(presence, Instant::now());
    }

    /// Records a presence report at an explicit time (tests).
    pub(crate) fn record_at(&self, presence: ClientPresence, now: Instant) {
        let mut tabs = self.tabs.lock().expect("presence lock poisoned");
        tabs.retain(|_, state| {
            now.saturating_duration_since(state.last_seen) <= PRESENCE_RETENTION
        });
        let state = tabs
            .entry((presence.client_id, presence.tab_id))
            .or_insert_with(|| TabState {
                visible: false,
                focused: false,
                focused_session_id: None,
                last_seen: now,
                last_active: None,
            });
        state.visible = presence.visible;
        state.focused = presence.focused;
        state.focused_session_id = presence.focused_session_id;
        state.last_seen = now;
        if presence.visible && presence.focused {
            state.last_active = Some(now);
        }
    }

    /// Removes a tab when its WS connection closes.
    pub(crate) fn clear(&self, client_id: &ClientId, tab_id: &TabId) {
        self.tabs
            .lock()
            .expect("presence lock poisoned")
            .remove(&(client_id.clone(), tab_id.clone()));
    }

    /// Clients with a `visible && focused` tab within the TTL.
    pub(crate) fn active_clients_at(&self, now: Instant) -> HashSet<ClientId> {
        let tabs = self.tabs.lock().expect("presence lock poisoned");
        tabs.iter()
            .filter(|(_, state)| {
                state.visible
                    && state.focused
                    && now.saturating_duration_since(state.last_seen) <= PRESENCE_TTL
            })
            .map(|((client_id, _), _)| client_id.clone())
            .collect()
    }

    /// Clients that were active within `window` (including tabs that are now hidden).
    pub(crate) fn recent_clients_at(&self, now: Instant, window: Duration) -> HashSet<ClientId> {
        let tabs = self.tabs.lock().expect("presence lock poisoned");
        tabs.iter()
            .filter(|(_, state)| {
                state
                    .last_active
                    .is_some_and(|last_active| now.saturating_duration_since(last_active) <= window)
            })
            .map(|((client_id, _), _)| client_id.clone())
            .collect()
    }

    /// The client whose tab was active most recently.
    pub(crate) fn most_recent_client_at(&self, now: Instant) -> Option<ClientId> {
        let tabs = self.tabs.lock().expect("presence lock poisoned");
        tabs.iter()
            .filter(|(_, state)| {
                state.last_active.is_some_and(|last_active| {
                    now.saturating_duration_since(last_active) <= PRESENCE_RETENTION
                })
            })
            .max_by_key(|(_, state)| state.last_active)
            .map(|((client_id, _), _)| client_id.clone())
    }

    /// Clients with an active tab that currently has `session_id` open.
    pub(crate) fn clients_focused_on_at(
        &self,
        session_id: &SessionId,
        now: Instant,
    ) -> HashSet<ClientId> {
        let tabs = self.tabs.lock().expect("presence lock poisoned");
        tabs.iter()
            .filter(|(_, state)| {
                state.visible
                    && state.focused
                    && now.saturating_duration_since(state.last_seen) <= PRESENCE_TTL
                    && state.focused_session_id.as_ref() == Some(session_id)
            })
            .map(|((client_id, _), _)| client_id.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn presence(client: &str, tab: &str, visible: bool, focused: bool) -> ClientPresence {
        ClientPresence {
            client_id: ClientId::new(client),
            tab_id: TabId::new(tab),
            visible,
            focused,
            focused_session_id: None,
        }
    }

    #[test]
    fn active_requires_visible_focused_and_fresh() {
        let registry = PresenceRegistry::default();
        let now = Instant::now();
        registry.record_at(presence("client-a", "tab-1", true, true), now);
        registry.record_at(presence("client-b", "tab-2", true, false), now);
        registry.record_at(presence("client-c", "tab-3", false, true), now);

        let active = registry.active_clients_at(now);
        assert!(active.contains(&ClientId::new("client-a")));
        assert!(!active.contains(&ClientId::new("client-b")));
        assert!(!active.contains(&ClientId::new("client-c")));

        let stale = now + PRESENCE_TTL + Duration::from_secs(1);
        assert!(registry.active_clients_at(stale).is_empty());
    }

    #[test]
    fn one_active_tab_makes_the_client_active() {
        let registry = PresenceRegistry::default();
        let now = Instant::now();
        registry.record_at(presence("client-a", "tab-1", false, false), now);
        registry.record_at(presence("client-a", "tab-2", true, true), now);
        assert!(
            registry
                .active_clients_at(now)
                .contains(&ClientId::new("client-a"))
        );
    }

    #[test]
    fn recent_includes_a_client_that_was_active_then_blurred() {
        let registry = PresenceRegistry::default();
        let now = Instant::now();
        registry.record_at(presence("client-a", "tab-1", true, true), now);
        registry.record_at(
            presence("client-a", "tab-1", true, false),
            now + Duration::from_secs(10),
        );

        let later = now + Duration::from_secs(60);
        assert!(registry.active_clients_at(later).is_empty());
        assert!(
            registry
                .recent_clients_at(later, Duration::from_secs(600))
                .contains(&ClientId::new("client-a"))
        );
        assert!(
            registry
                .recent_clients_at(now + Duration::from_secs(700), Duration::from_secs(600))
                .is_empty()
        );
    }

    #[test]
    fn most_recent_client_uses_the_latest_activity() {
        let registry = PresenceRegistry::default();
        let now = Instant::now();
        registry.record_at(presence("client-a", "tab-1", true, true), now);
        registry.record_at(
            presence("client-b", "tab-2", true, true),
            now + Duration::from_secs(5),
        );
        assert_eq!(
            registry.most_recent_client_at(now + Duration::from_secs(10)),
            Some(ClientId::new("client-b"))
        );
    }

    #[test]
    fn focused_session_is_reported_only_for_active_tabs() {
        let registry = PresenceRegistry::default();
        let now = Instant::now();
        let session = SessionId::new("session-1");
        let mut first = presence("client-a", "tab-1", true, true);
        first.focused_session_id = Some(session.clone());
        registry.record_at(first, now);
        let mut second = presence("client-b", "tab-2", true, false);
        second.focused_session_id = Some(session.clone());
        registry.record_at(second, now);

        let focused = registry.clients_focused_on_at(&session, now);
        assert!(focused.contains(&ClientId::new("client-a")));
        assert!(!focused.contains(&ClientId::new("client-b")));
    }

    #[test]
    fn clearing_a_tab_removes_it() {
        let registry = PresenceRegistry::default();
        let now = Instant::now();
        registry.record_at(presence("client-a", "tab-1", true, true), now);
        registry.clear(&ClientId::new("client-a"), &TabId::new("tab-1"));
        assert!(registry.active_clients_at(now).is_empty());
    }
}
