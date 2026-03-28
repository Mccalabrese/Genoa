use std::sync::OnceLock;
use tokio::sync::Mutex;
use anyhow::{Result, Context};
use yahoo_finance_api::YahooConnector;
use time::OffsetDateTime;
use serde::{Deserialize, Serialize};
use futures::future::join_all;
use crate::config::{get_config_path, load_config};
use crate::app::{StockDetails, MarketStatus};


#[derive(Debug, Deserialize)]
pub struct FinnhubQuote {
    #[serde(rename = "c")]
    pub price: f64,

    #[serde(rename = "dp")]
    pub percent: f64,
}

#[derive(Debug, Serialize)]
pub struct WaybarOutput {
    pub text: String,
    pub tooltip: String,
    pub class: String,
}

#[derive(Debug, Deserialize)]
pub struct YahooSearchResponse {
    pub quotes: Vec<YahooSearchResult>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct YahooSearchResult {
    pub symbol: String,

    #[serde(rename = "shortname")]
    pub name: Option<String>,

    #[serde(rename = "quoteType")]
    pub quote_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct YahooQuoteResponse {
    #[serde(rename = "quoteResponse")]
    quote_response: QuoteResult,
}

#[derive(Debug, Deserialize)]
struct QuoteResult {
    result: Vec<YahooQuote>,
}

#[derive(Debug, Deserialize)]
struct YahooQuote {
    #[serde(rename = "marketCap")]
    market_cap: Option<f64>,

    #[serde(rename = "netAssets")]
    net_assets: Option<f64>,

    #[serde(rename = "trailingPE")]
    pe_ratio: Option<f64>,

    #[serde(rename = "dividendYield")] 
    dividend_yield: Option<f64>, 

    #[serde(rename = "trailingAnnualDividendYield")]
    trailing_yield: Option<f64>,

    #[serde(rename = "fiftyTwoWeekHigh")]
    high_52w: Option<f64>,

    #[serde(rename = "fiftyTwoWeekLow")]
    low_52w: Option<f64>,
    #[serde(rename = "regularMarketPrice")]
    regular_market_price: Option<f64>,
    
    #[serde(rename = "ytdReturn")]
    ytd_return: Option<f64>,

    #[serde(rename = "fiftyTwoWeekChangePercent")]
    fifty_two_week_change: Option<f64>,

    symbol: String,
}
// Global cache for the yahoo crumb to avoid re-fetching each request.
static YAHOO_CRUMB: OnceLock<Mutex<Option<String>>> = OnceLock::new();

async fn get_yahoo_crumb(client: &reqwest::Client) -> Result<String> {
    // Check cache first
    let mutex = YAHOO_CRUMB.get_or_init(|| Mutex::new(None));
    let mut lock = mutex.lock().await;
    
    if let Some(c) = &*lock {
        return Ok(c.clone());
    }

    // Handshake - Get Cookies
    let _ = client.get("https://fc.yahoo.com")
        .header("Accept", "*/*")
        .send().await;

    // Handshake - Ask for the Crumb
    let resp = client.get("https://query1.finance.yahoo.com/v1/test/getcrumb")
        .header("Accept", "*/*")
        .send()
        .await?;
    // ... error handling ...
    if !resp.status().is_success() {
        return Err(anyhow::anyhow!("Failed to get crumb: {}", resp.status()));
    }

    let crumb = resp.text().await?;
    
    // Handshake - Cache it
    *lock = Some(crumb.clone());
    
    Ok(crumb)
}

/// Fetches search results from Yahoo Finance's search endpoint.
/// Handles basic symbol search.
pub async fn search_ticker(client: &reqwest::Client, query: &str) -> Result<Vec<YahooSearchResult>> {
    let url = format!(
        "https://query2.finance.yahoo.com/v1/finance/search?q={}&lang=en-US",
        query
    );

    // send the GET request
    let resp = client
        .get(&url)
        .header("Accept", "*/*")
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(anyhow::anyhow!("Search failed: {}", resp.status()));
    }

    let data: YahooSearchResponse = resp.json().await?;
    Ok(data.quotes)
}

/// Fetches detailed metrics (P/E, Yield, etc.) from Yahoo's v7 endpoint.
/// Handles the differences between Stocks (using Dividend Yield) and ETFs (using 12-Mo Yield).
pub async fn fetch_details(client: &reqwest::Client, symbol: &str, _key: &str) -> Result<StockDetails> {
    let crumb = get_yahoo_crumb(client).await?; 
    
    let url = format!(
        "https://query1.finance.yahoo.com/v7/finance/quote?symbols={}&crumb={}",
        symbol, crumb
    );

    let resp = client.get(&url).send().await?;
    
    if !resp.status().is_success() {
        return Err(anyhow::anyhow!("Yahoo Error: {}", resp.status()));
    }

    let data: YahooQuoteResponse = resp.json().await?;
    
    if data.quote_response.result.is_empty() {
        return Err(anyhow::anyhow!("No data found"));
    }

    let q = &data.quote_response.result[0];
    
    // Polymorphic Field Logic:
    // Different asset classes (Stocks vs ETFs) store yield in different fields.
    // We try them in order of specificity.
    let final_yield = if let Some(y) = q.dividend_yield {
        Some(y)
    } else { q.trailing_yield.map(|y| y * 100.0) };

    // Fallback for Market Cap (ETFs use Net Assets)
    let mkt_cap = q.market_cap.or(q.net_assets).unwrap_or(0.0) as u64;

    // PERFORMANCE LOGIC:
    // 1. Try YTD (Common for ETFs, usually formatted as 5.0 for 5%)
    // 2. Try 52W Change (Common for Stocks, usually formatted as 0.05 for 5%)
    let perf = if let Some(ytd) = q.ytd_return {
        Some(ytd)
    } else { q.fifty_two_week_change };

    Ok(StockDetails {
        market_cap: mkt_cap,
        pe_ratio: q.pe_ratio,
        dividend_yield: final_yield,
        high_52w: q.high_52w.unwrap_or(0.0),
        low_52w: q.low_52w.unwrap_or(0.0),
        year_return: perf,
    })
}
/// Fetches real-time stock quote from Finnhub API.
pub async fn fetch_quote(client: &reqwest::Client, symbol: &str, key: &str) -> Result<FinnhubQuote> {
    let url = format!(
        "https://finnhub.io/api/v1/quote?symbol={}&token={}",
        symbol, key
    );
    let resp = client.get(&url).send().await?;
    if !resp.status().is_success() {
        return Err(anyhow::anyhow!("Failed to fetch quote: HTTP {}", resp.status()));
    }
    let quote: FinnhubQuote = resp.json().await?;
    Ok(quote)
}
/// Fetches historical stock data from Yahoo Finance API.
/// The data points are returned as a vector of (timestamp, close price) tuples.
/// Used by the charting component.
pub async fn fetch_history(_client: &reqwest::Client, symbol: &str, _key: &str) -> Result<Vec<(f64, f64)>> {
    let provider = YahooConnector::new()?;
    let end = OffsetDateTime::now_utc();
    let start = end - time::Duration::days(365);
    let response = provider.get_quote_history(symbol, start, end).await
        .context("Yaho API Error")?;
    let quotes = response.quotes().context("No quotes in response")?;
    let points: Vec<(f64, f64)> = quotes.iter()
        .map(|q| (q.timestamp as f64, q.close))
        .collect();
    if points.is_empty() {
        return Err(anyhow::anyhow!("History data is empty"));
    }
    Ok(points)
}
/// Uses the Finnhub API to fetch real-time stock quotes for all symbols
/// Outputs the data in Waybar-compatible JSON format.
pub async fn run_waybar_mode(client: &reqwest::Client) -> Result<()> {
    let config_path = get_config_path()?;
    let config = load_config(&config_path)?;
    let api_key = match &config.api_key {
        Some(k) => k,
        None => {
            // OUTPUT A CLICKABLE JSON SO USER KNOWS TO SETUP
            let output = WaybarOutput {
                text: "⚠️ Finance Setup".to_string(),
                tooltip: "Click to launch setup and enter API Key".to_string(),
                class: "warning".to_string(),
            };
            println!("{}", serde_json::to_string(&output)?);
            return Ok(());
        }
    };

    let futures: Vec<_> = config.stocks.iter().filter(|s| s.sidebar.unwrap_or(false)).filter_map(|s| s.symbol.as_ref()).map(|sym| {
            let client = client.clone();
            let key = api_key.clone();
            let sym = sym.clone();
            async move {
                let q = fetch_quote(&client, &sym, &key).await;
                (sym, q)
            }
        }).collect();

    let results = join_all(futures).await;
    let mut text_parts = Vec::new();
    let mut tooltip_parts = Vec::new();
    for (symbol, result) in results {
        match result {
            Ok(quote) => {
                let (color, icon) = if quote.percent >= 0.0 {
                    ("#a6e3a1", "")
                } else {
                    ("#f38ba8", "")
                };
                let part = format!(
                    "<span color='{}'>{} {:.2} {}</span>",
                    color, symbol, quote.price, icon
                );
                text_parts.push(part);
                tooltip_parts.push(format!(
                    "<span color='{}'>{}: ${:.2} ({:.2}%)</span>", 
                    color, symbol, quote.price, quote.percent
                ));
            }
            Err(_) => {
                text_parts.push(format!("<span color='#6c7086'>{} ???</span>", symbol));
            }
        }
    }
    let output = WaybarOutput {
        text: text_parts.join(" "),
        tooltip: tooltip_parts.join("\n"),
        class: "finance".to_string(),
    };
    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}
/// Fetches market status including yields for 10Y, 5Y, and 3M Treasuries from Yahoo Finance.
/// Used for displaying yield data and yield curve in app's top banner.
pub async fn fetch_market_status(client: &reqwest::Client) -> Result<MarketStatus> {
    // 1. Get Crumb
    let crumb = get_yahoo_crumb(client).await?;

    // 2. Batch Request
    let url = format!(
        "https://query1.finance.yahoo.com/v7/finance/quote?symbols=^TNX,^FVX,^IRX&crumb={}",
        crumb
    );

    let resp = client.get(&url).send().await?;
    
    if !resp.status().is_success() {
        return Err(anyhow::anyhow!("Yields Error: {}", resp.status()));
    }

    let data: YahooQuoteResponse = resp.json().await?;
    let results = data.quote_response.result;

    // 3. Map results
    // We need to find which is which because lists aren't always ordered
    let mut y10 = 0.0;
    let mut y5 = 0.0;
    let mut y3m = 0.0;

    for q in results {
        let val = q.regular_market_price.unwrap_or(0.0); // We need to add regularMarketPrice to YahooQuote struct!
        match q.symbol.as_str() {
            "^TNX" => y10 = val,
            "^FVX" => y5 = val,
            "^IRX" => y3m = val,
            _ => {}
        }
    }

    Ok(MarketStatus {
        yield_10y: y10,
        yield_5y: y5,
        yield_3m: y3m,
    })
}
