use crate::ui::AppEvent;
use ratatui::style::Color;
use ratatui::widgets::ListState;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::Sender;

use crate::config::{MAX_SIDEBAR_QUOTES, MAX_WATCHLIST_STOCKS, StockStruct, normalize_symbol};
use crate::network::{MarketQuote, YahooSearchResult};

/// Defines the input state of the TUI.
/// We use a state machine approach to change keybindings based on context.
#[derive(Debug, PartialEq)]
pub enum InputMode {
    Normal,  // Navigation and viewing
    Editing, // Typing in the search bar
}

#[derive(Debug, PartialEq, Eq)]
pub enum SidebarToggleResult {
    Enabled,
    Disabled,
    LimitReached,
    NoSelection,
}
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Config {
    pub stocks: Vec<StockStruct>,
}
// Default configuration for new users
impl Default for Config {
    fn default() -> Self {
        Self {
            stocks: vec![
                StockStruct {
                    symbol: "SPY".into(),
                    sidebar: true,
                },
                StockStruct {
                    symbol: "QQQ".into(),
                    sidebar: true,
                },
                StockStruct {
                    symbol: "BTC-USD".into(),
                    sidebar: true,
                },
            ],
        }
    }
}
/// Defines the data for the detailed stock view.
#[derive(Debug, Clone)]
pub struct StockDetails {
    pub market_cap: u64,
    pub pe_ratio: Option<f64>,
    pub dividend_yield: Option<f64>,
    pub high_52w: f64,
    pub low_52w: f64,
    pub year_return: Option<f64>,
}
/// Defines the current market status (bond yields, yield curve etc)
#[derive(Debug, Clone)]
pub struct MarketStatus {
    pub yield_10y: f64,
    pub yield_5y: f64,
    pub yield_3m: f64,
}
/// Calculation for yield curve.
impl MarketStatus {
    // Calculate the 10Y - 3M spread
    pub fn spread_10y_3m(&self) -> f64 {
        self.yield_10y - self.yield_3m
    }
}
/// Holds the runtime state of the TUI application.
pub struct App {
    pub stocks: Vec<StockStruct>,
    pub should_quit: bool,
    pub state: ListState, // tracks the selected item in the stock list
    // Cached Data
    pub current_quote: Option<MarketQuote>,
    pub stock_history: Option<Vec<(f64, f64)>>,
    pub details: Option<StockDetails>,
    pub search_results: Vec<YahooSearchResult>,
    pub search_state: ListState,
    pub market_status: Option<MarketStatus>,

    // Input Handling
    pub input: String,
    pub input_mode: InputMode,

    // UI Feedback
    pub message: String,
    pub message_color: Color,
}

impl App {
    pub fn new(
        config: Config,
        message: String,
        message_color: Color,
        stock_history: Option<Vec<(f64, f64)>>,
    ) -> Self {
        let mut state = ListState::default();
        state.select(Some(0));
        let sidebar_count = config.stocks.iter().filter(|stock| stock.sidebar).count();
        let (message, message_color) = if sidebar_count > MAX_SIDEBAR_QUOTES {
            (
                format!("Sidebar shows the first {MAX_SIDEBAR_QUOTES} selected tickers"),
                Color::Yellow,
            )
        } else {
            (message, message_color)
        };
        Self {
            stocks: config.stocks,
            should_quit: false,
            state,
            current_quote: None,
            input: String::new(),
            input_mode: InputMode::Normal,
            message,
            message_color,
            stock_history,
            details: None,
            search_results: vec![],
            search_state: ListState::default(),
            market_status: None,
        }
    }
    /// Moves the selection index down, wrapping around if necessary.
    pub fn next(&mut self) {
        if self.stocks.is_empty() {
            return;
        }
        let i = match self.state.selected() {
            Some(i) => (i + 1) % self.stocks.len(),
            None => 0,
        };
        self.state.select(Some(i));
    }

    pub fn previous(&mut self) {
        if self.stocks.is_empty() {
            return;
        }
        let i = match self.state.selected() {
            Some(i) => (i + self.stocks.len() - 1) % self.stocks.len(),
            None => 0,
        };
        self.state.select(Some(i));
    }

    /// Helper to export state for saving
    pub fn to_config(&self) -> Config {
        Config {
            stocks: self.stocks.clone(),
        }
    }

    pub fn delete(&mut self) {
        if let Some(selected) = self.state.selected() {
            if self.stocks.is_empty() {
                return;
            }
            self.stocks.remove(selected);

            if self.stocks.is_empty() {
                self.state.select(None);
            } else if selected >= self.stocks.len() {
                self.state.select(Some(self.stocks.len() - 1));
            }
        }
    }

    pub fn next_search(&mut self) {
        if self.search_results.is_empty() {
            return;
        }
        let i = match self.search_state.selected() {
            Some(i) => (i + 1) % self.search_results.len(),
            None => 0,
        };
        self.search_state.select(Some(i));
    }

    pub fn previous_search(&mut self) {
        if self.search_results.is_empty() {
            return;
        }
        let i = match self.search_state.selected() {
            Some(i) => (i + self.search_results.len() - 1) % self.search_results.len(),
            None => 0,
        };
        self.search_state.select(Some(i));
    }

    pub fn toggle_sidebar_view(&mut self) -> SidebarToggleResult {
        let Some(selected) = self.state.selected() else {
            return SidebarToggleResult::NoSelection;
        };
        let Some(sidebar_enabled) = self.stocks.get(selected).map(|stock| stock.sidebar) else {
            return SidebarToggleResult::NoSelection;
        };

        if sidebar_enabled {
            self.stocks[selected].sidebar = false;
            return SidebarToggleResult::Disabled;
        }

        if self.sidebar_visible_count() >= MAX_SIDEBAR_QUOTES {
            return SidebarToggleResult::LimitReached;
        }

        self.stocks[selected].sidebar = true;
        SidebarToggleResult::Enabled
    }

    fn sidebar_visible_count(&self) -> usize {
        self.stocks.iter().filter(|stock| stock.sidebar).count()
    }

    ///Handles adding a stock and triggers data fetch
    pub fn handle_confirm_selection(&mut self, tx: &Sender<AppEvent>, client: &reqwest::Client) {
        let new_symbol = if let Some(idx) = self.search_state.selected() {
            self.search_results[idx].symbol.clone()
        } else {
            self.input.clone()
        };

        let Some(new_symbol) = normalize_symbol(&new_symbol) else {
            self.message = "Enter a valid ticker symbol".to_string();
            self.message_color = Color::Yellow;
            return;
        };

        if self.stocks.iter().any(|s| s.symbol == new_symbol) {
            self.message = format!("{} exists!", new_symbol);
            self.message_color = Color::Yellow;
        } else if self.stocks.len() >= MAX_WATCHLIST_STOCKS {
            self.message = format!("Watchlist limit reached ({MAX_WATCHLIST_STOCKS})");
            self.message_color = Color::Yellow;
        } else {
            let show_in_sidebar = self.sidebar_visible_count() < MAX_SIDEBAR_QUOTES;
            if show_in_sidebar {
                self.message_color = Color::Green;
                self.message = format!("Added {new_symbol}");
            } else {
                self.message_color = Color::Yellow;
                self.message = format!(
                    "Added {new_symbol}; Sidebar quote limit reached ({MAX_SIDEBAR_QUOTES})"
                );
            }

            self.state.select(Some(self.stocks.len() - 1));

            // Trigger background work
            self.trigger_fetch(new_symbol.clone(), tx, client);
            self.stocks.push(StockStruct {
                symbol: new_symbol,
                sidebar: show_in_sidebar,
            });
            let tx_clone = tx.clone();
            tokio::spawn(async move {
                let _ = tx_clone.send(AppEvent::SaveConfig).await;
            });
        }

        self.input.clear();
        self.search_results.clear();
        self.input_mode = InputMode::Normal;
    }

    /// Centralized fetch logic to avoid code duplication
    pub fn trigger_fetch(&self, symbol: String, tx: &Sender<AppEvent>, client: &reqwest::Client) {
        let client = client.clone();
        let tx = tx.clone();
        let symbol = symbol.clone();

        tokio::spawn(async move {
            let q_res = crate::network::fetch_quote(&client, &symbol).await;
            let _ = tx.send(AppEvent::QuoteFetched(symbol.clone(), q_res)).await;

            let h_res = crate::network::fetch_history(&client, &symbol).await;
            let _ = tx
                .send(AppEvent::HistoryFetched(symbol.clone(), h_res))
                .await;

            let d_res = crate::network::fetch_details(&client, &symbol).await;
            let _ = tx
                .send(AppEvent::DetailsFetched(symbol.clone(), d_res))
                .await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stock(symbol: impl Into<String>, sidebar: bool) -> StockStruct {
        StockStruct {
            symbol: symbol.into(),
            sidebar,
        }
    }

    #[test]
    fn sidebar_toggle_rejects_an_additional_visible_ticker() {
        let mut stocks: Vec<_> = (0..MAX_SIDEBAR_QUOTES)
            .map(|index| stock(format!("T{index}"), true))
            .collect();
        stocks.push(stock("HIDDEN", false));
        let mut app = App::new(Config { stocks }, "Ready".to_string(), Color::Green, None);
        app.state.select(Some(MAX_SIDEBAR_QUOTES));

        assert_eq!(app.toggle_sidebar_view(), SidebarToggleResult::LimitReached);
        assert!(!app.stocks[MAX_SIDEBAR_QUOTES].sidebar);
    }

    #[test]
    fn sidebar_toggle_always_allows_disabling_an_existing_ticker() {
        let stocks: Vec<_> = (0..=MAX_SIDEBAR_QUOTES)
            .map(|index| stock(format!("T{index}"), true))
            .collect();
        let mut app = App::new(Config { stocks }, "Ready".to_string(), Color::Green, None);
        app.state.select(Some(MAX_SIDEBAR_QUOTES));

        assert_eq!(app.toggle_sidebar_view(), SidebarToggleResult::Disabled);
        assert!(!app.stocks[MAX_SIDEBAR_QUOTES].sidebar);
    }

    #[test]
    fn app_warns_when_an_existing_config_exceeds_the_sidebar_limit() {
        let stocks: Vec<_> = (0..=MAX_SIDEBAR_QUOTES)
            .map(|index| stock(format!("T{index}"), true))
            .collect();
        let app = App::new(Config { stocks }, "Ready".to_string(), Color::Green, None);

        assert_eq!(
            app.message,
            format!("Sidebar shows the first {MAX_SIDEBAR_QUOTES} selected tickers")
        );
    }
}
