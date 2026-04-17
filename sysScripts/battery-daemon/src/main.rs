use futures_util::StreamExt;
use std::process::Command;
use tokio::time::{Duration, sleep};
use zbus::{Connection, proxy};

#[proxy(
    default_service = "org.freedesktop.UPower",
    default_path = "/org/freedesktop/UPower/devices/DisplayDevice",
    interface = "org.freedesktop.UPower.Device"
)]
trait UPowerDevice {
    #[zbus(property)]
    fn percentage(&self) -> zbus::Result<f64>;

    #[zbus(property)]
    fn state(&self) -> zbus::Result<u32>;
}

#[tokio::main]
async fn main() -> zbus::Result<()> {
    let mut warning_15 = false;
    let mut warning_10 = false;

    loop {
        let connection = match Connection::system().await {
            Ok(val) => val,
            Err(e) => {
                eprintln!("Failed to connect to D-Bus: {}", e);
                sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        let proxy = match UPowerDeviceProxy::new(&connection).await {
            Ok(val) => val,
            Err(e) => {
                eprintln!("Failed to create proxy: {}", e);
                sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        let battery_life = match proxy.percentage().await {
            Ok(val) => val,
            Err(e) => {
                eprintln!("Failed to get battery percentage: {}", e);
                sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        let battery_state = match proxy.state().await {
            Ok(val) => val,
            Err(e) => {
                eprintln!("Failed to get battery state: {}", e);
                sleep(Duration::from_secs(5)).await;
                continue;
            }
        };

        if battery_life <= 15.0 && battery_state == 2 {
            // battery warning upon boot
            if let Err(e) = Command::new("/usr/bin/notify-send")
                .arg("Warning: Battery Low")
                .arg("Shuts down at 5%")
                .spawn()
            {
                eprintln!("Failed to send notification: {}", e);
            }
            warning_15 = true;
            if battery_life <= 10.0 {
                warning_10 = true;
            }
        }

        let mut battery_influx = proxy.receive_percentage_changed().await;

        while let Some(event) = battery_influx.next().await {
            let updated_life = match event.get().await {
                Ok(val) => val,
                Err(e) => {
                    eprintln!("Failed to get updated battery percentage: {}", e);
                    break;
                }
            };
            let battery_state = match proxy.state().await {
                Ok(val) => val,
                Err(e) => {
                    eprintln!("Failed to get battery state: {}", e);
                    break;
                }
            };
            if updated_life <= 15.0 && battery_state == 2 && !warning_15 {
                // battery warning at 15%
                if let Err(e) = Command::new("/usr/bin/notify-send")
                    .arg("Battery Warning 15%")
                    .arg("Shuts down at 5%")
                    .spawn()
                {
                    eprintln!("Failed to send notification: {}", e);
                }
                warning_15 = true;
            }
            if updated_life <= 10.0 && battery_state == 2 && !warning_10 {
                // battery warning at 10%
                if let Err(e) = Command::new("/usr/bin/notify-send")
                    .arg("Battery Warning 10%")
                    .arg("Shuts down at 5%\nSAVE WORK NOW")
                    .spawn()
                {
                    eprintln!("Failed to send notification: {}", e);
                }
                warning_10 = true;
            }
            if battery_state != 2 {
                // prevents firing warning when plugged in but resets after unplugging
                warning_15 = false;
                warning_10 = false;
            }
        }
        sleep(Duration::from_secs(30)).await;
    }
    #[allow(unreachable_code)]
    Ok(())
}
