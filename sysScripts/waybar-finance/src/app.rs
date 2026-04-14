use crate::ui::AppEvent;
use ratatui::style::Color;
use ratatui::widgets::ListState;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::Sender;

use crate::app::InputMode::Normal;
use crate::config::StockStruct;
use crate::network::{FinnhubQuote, YahooSearchResult};

/// Defines the input state of the TUI.
/// We use a state machine approach to change keybindings based on context.
#[derive(Debug, PartialEq)]
pub enum InputMode {
    Normal,   //Navigation and viewing
    Editing,  // Typing in the search bar
    KeyEntry, // Force-prompt for API key on first run
}
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Config {
    pub stocks: Vec<StockStruct>,
    pub api_key: Option<String>,
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
            api_key: None,
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
    pub api_key: Option<String>,

    // Cached Data
    pub current_quote: Option<FinnhubQuote>,
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
        // Detect if this is a first run (missing API key) and force KeyEntry mode.
        let (input_mode, msg, color) = if config.api_key.is_some() {
            (Normal, message, message_color)
        } else {
            (
                InputMode::KeyEntry,
                "Welcome! Please enter your Finnhub API Key.".to_string(),
                Color::Yellow,
            )
        };
        Self {
            stocks: config.stocks,
            should_quit: false,
            state,
            api_key: config.api_key,
            current_quote: None,
            input: String::new(),
            input_mode,
            message: msg,
            message_color: color,
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
            api_key: self.api_key.clone(),
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

    pub fn toggle_sidebar_view(&mut self) {
        if let Some(selected) = self.state.selected()
            && let Some(stock) = self.stocks.get_mut(selected) {
                stock.sidebar = !stock.sidebar;
            }
    }

    ///Handles adding a stock and triggers data fetch
    pub fn handle_confirm_selection(&mut self, tx: &Sender<AppEvent>, client: &reqwest::Client) {
        let new_symbol = if let Some(idx) = self.search_state.selected() {
            self.search_results[idx].symbol.clone()
        } else {
            self.input.trim().to_uppercase()
        };

        if new_symbol.is_empty() {
            return;
        }

        if self.stocks.iter().any(|s| s.symbol == new_symbol) {
            self.message = format!("{} exists!", new_symbol);
            self.message_color = Color::Yellow;
        } else {
            self.message_color = Color::Green;
            self.message = format!("Added {}", &new_symbol);

            self.state.select(Some(self.stocks.len() - 1));

            // Trigger background work
            self.trigger_fetch(new_symbol.clone(), tx, client);
            self.stocks.push(StockStruct {
                symbol: new_symbol,
                sidebar: true,
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
        let api_key = self.api_key.clone().unwrap_or_default();
        let symbol = symbol.clone();

        tokio::spawn(async move {
            let q_res = crate::network::fetch_quote(&client, &symbol, &api_key).await;
            let _ = tx.send(AppEvent::QuoteFetched(symbol.clone(), q_res)).await;

            let h_res = crate::network::fetch_history(&client, &symbol, &api_key).await;
            let _ = tx
                .send(AppEvent::HistoryFetched(symbol.clone(), h_res))
                .await;

            let d_res = crate::network::fetch_details(&client, &symbol, &api_key).await;
            let _ = tx
                .send(AppEvent::DetailsFetched(symbol.clone(), d_res))
                .await;
        });
    }
}
