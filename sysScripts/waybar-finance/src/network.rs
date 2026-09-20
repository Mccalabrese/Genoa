use crate::app::Config;
use crate::app::{MarketStatus, StockDetails};
use anyhow::{Context, Result};
use futures::future::join_all;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use time::OffsetDateTime;
use tokio::sync::Mutex;

#[derive(Debug)]
pub struct MarketQuote {
    pub price: f64,
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

#[derive(Debug, Deserialize)]
struct YahooChartResponse {
    chart: YahooChart,
}

#[derive(Debug, Deserialize)]
struct YahooChart {
    result: Option<Vec<YahooChartResult>>,
    error: Option<YahooChartError>,
}

#[derive(Debug, Deserialize)]
struct YahooChartError {
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct YahooChartResult {
    meta: YahooChartMeta,
    timestamp: Option<Vec<i64>>,
    indicators: YahooChartIndicators,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct YahooChartMeta {
    regular_market_price: Option<f64>,
    chart_previous_close: Option<f64>,
    previous_close: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct YahooChartIndicators {
    quote: Vec<YahooChartQuote>,
}

#[derive(Debug, Deserialize)]
struct YahooChartQuote {
    close: Vec<Option<f64>>,
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
    let _ = client
        .get("https://fc.yahoo.com")
        .header("Accept", "*/*")
        .send()
        .await;

    // Handshake - Ask for the Crumb
    let resp = client
        .get("https://query1.finance.yahoo.com/v1/test/getcrumb")
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
pub async fn search_ticker(
    client: &reqwest::Client,
    query: &str,
) -> Result<Vec<YahooSearchResult>> {
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
pub async fn fetch_details(client: &reqwest::Client, symbol: &str) -> Result<StockDetails> {
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
    } else {
        q.trailing_yield.map(|y| y * 100.0)
    };

    // Fallback for Market Cap (ETFs use Net Assets)
    let mkt_cap = q.market_cap.or(q.net_assets).unwrap_or(0.0) as u64;

    // PERFORMANCE LOGIC:
    // 1. Try YTD (Common for ETFs, usually formatted as 5.0 for 5%)
    // 2. Try 52W Change (Common for Stocks, usually formatted as 0.05 for 5%)
    let perf = if let Some(ytd) = q.ytd_return {
        Some(ytd)
    } else {
        q.fifty_two_week_change
    };

    Ok(StockDetails {
        market_cap: mkt_cap,
        pe_ratio: q.pe_ratio,
        dividend_yield: final_yield,
        high_52w: q.high_52w.unwrap_or(0.0),
        low_52w: q.low_52w.unwrap_or(0.0),
        year_return: perf,
    })
}
/// Fetches a current quote from Yahoo Finance without an API key.
pub async fn fetch_quote(client: &reqwest::Client, symbol: &str) -> Result<MarketQuote> {
    let chart = fetch_yahoo_chart(
        client,
        symbol,
        &[
            ("interval", "1m"),
            ("range", "1d"),
            ("events", "div|split|capitalGains"),
        ],
    )
    .await?;
    let metadata = chart.meta;

    let price = metadata
        .regular_market_price
        .ok_or_else(|| anyhow::anyhow!("Yahoo Finance returned no current price for {symbol}"))?;
    let previous_close = metadata
        .previous_close
        .or(metadata.chart_previous_close)
        .ok_or_else(|| anyhow::anyhow!("Yahoo Finance returned no previous close for {symbol}"))?;

    market_quote_from_prices(price, previous_close)
}
/// Fetches historical stock data from Yahoo Finance API.
/// The data points are returned as a vector of (timestamp, close price) tuples.
/// Used by the charting component.
pub async fn fetch_history(client: &reqwest::Client, symbol: &str) -> Result<Vec<(f64, f64)>> {
    let end = OffsetDateTime::now_utc();
    let start = end - time::Duration::days(365);
    let start_timestamp = start.unix_timestamp().to_string();
    let end_timestamp = end.unix_timestamp().to_string();
    let chart = fetch_yahoo_chart(
        client,
        symbol,
        &[
            ("period1", start_timestamp.as_str()),
            ("period2", end_timestamp.as_str()),
            ("interval", "1d"),
            ("events", "div|split|capitalGains"),
        ],
    )
    .await?;
    history_points_from_chart(chart)
}
/// Fetches the Sidebar-visible quotes and outputs Waybar-compatible JSON.
/// The compatibility format lets existing external status-bar integrations
/// continue working while Genoa displays it in Sidebar.
pub async fn run_widget_mode(config: &Config, client: &reqwest::Client) -> Result<()> {
    let futures: Vec<_> = config
        .stocks
        .iter()
        .filter(|s| s.sidebar)
        .map(|s| {
            let sym = s.symbol.clone();
            async move {
                let q = fetch_quote(client, &sym).await;
                (sym, q)
            }
        })
        .collect();

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

async fn fetch_yahoo_chart(
    client: &reqwest::Client,
    symbol: &str,
    query: &[(&str, &str)],
) -> Result<YahooChartResult> {
    let url = yahoo_chart_url(symbol, query)?;

    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("Yahoo Finance could not load data for {symbol}"))?
        .error_for_status()
        .with_context(|| format!("Yahoo Finance rejected the request for {symbol}"))?;
    let response: YahooChartResponse = response
        .json()
        .await
        .with_context(|| format!("Yahoo Finance returned invalid data for {symbol}"))?;

    response
        .chart
        .result
        .and_then(|mut results| results.pop())
        .ok_or_else(|| {
            let detail = response
                .chart
                .error
                .and_then(|error| error.description)
                .unwrap_or_else(|| "no chart data returned".to_string());
            anyhow::anyhow!("Yahoo Finance could not load {symbol}: {detail}")
        })
}

fn yahoo_chart_url(symbol: &str, query: &[(&str, &str)]) -> Result<reqwest::Url> {
    let mut url = reqwest::Url::parse("https://query1.finance.yahoo.com/v8/finance/chart/")
        .expect("Yahoo Finance chart base URL is valid");
    url.path_segments_mut()
        .map_err(|()| anyhow::anyhow!("Yahoo Finance chart URL cannot accept a ticker path"))?
        .pop_if_empty()
        .push(symbol);
    {
        let mut query_pairs = url.query_pairs_mut();
        query_pairs.append_pair("symbol", symbol);
        for (key, value) in query {
            query_pairs.append_pair(key, value);
        }
    }
    Ok(url)
}

fn market_quote_from_prices(price: f64, previous_close: f64) -> Result<MarketQuote> {
    if !price.is_finite() || !previous_close.is_finite() || previous_close == 0.0 {
        anyhow::bail!("Yahoo Finance returned invalid price data");
    }

    Ok(MarketQuote {
        price,
        percent: ((price - previous_close) / previous_close) * 100.0,
    })
}

fn history_points_from_chart(chart: YahooChartResult) -> Result<Vec<(f64, f64)>> {
    let timestamps = chart
        .timestamp
        .context("Yahoo Finance returned no timestamps")?;
    let closes = chart
        .indicators
        .quote
        .first()
        .context("Yahoo Finance returned no quote data")?
        .close
        .iter();
    let points: Vec<(f64, f64)> = timestamps
        .into_iter()
        .zip(closes)
        .filter_map(|(timestamp, close)| close.map(|close| (timestamp as f64, close)))
        .collect();
    if points.is_empty() {
        return Err(anyhow::anyhow!("History data is empty"));
    }
    Ok(points)
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

#[cfg(test)]
mod tests {
    use super::{
        YahooChartResponse, history_points_from_chart, market_quote_from_prices, yahoo_chart_url,
    };

    #[test]
    fn current_quote_uses_the_previous_close_for_daily_change() {
        let quote = market_quote_from_prices(110.0, 100.0).unwrap();

        assert_eq!(quote.price, 110.0);
        assert_eq!(quote.percent, 10.0);
    }

    #[test]
    fn current_quote_rejects_an_invalid_previous_close() {
        assert!(market_quote_from_prices(110.0, 0.0).is_err());
    }

    #[test]
    fn chart_history_ignores_missing_close_values() {
        let response: YahooChartResponse = serde_json::from_str(
            r#"{
                "chart": {
                    "result": [{
                        "meta": {},
                        "timestamp": [100, 200, 300],
                        "indicators": {"quote": [{"close": [10.0, null, 12.5]}]}
                    }],
                    "error": null
                }
            }"#,
        )
        .unwrap();
        let chart = response.chart.result.unwrap().into_iter().next().unwrap();

        assert_eq!(
            history_points_from_chart(chart).unwrap(),
            vec![(100.0, 10.0), (300.0, 12.5)]
        );
    }

    #[test]
    fn chart_url_has_one_separator_before_the_ticker() {
        let url = yahoo_chart_url("SPY", &[("range", "1d")]).unwrap();

        assert_eq!(url.path(), "/v8/finance/chart/SPY");
        assert_eq!(url.query(), Some("symbol=SPY&range=1d"));
    }
}
