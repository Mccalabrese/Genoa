use std::fs;
use std::thread;
use std::time::Duration;
use std::process::Command;

fn main() {
    let battery_path = "/sys/class/power_supply/BAT0/capacity"; // path to check current battery capacity
    let status_path = "/sys/class/power_supply/BAT0/status"; // path to check the status (discharging, charging, etc)

    // These are for the popup warnings to ensure that they will show up no matter what
    let mut warning_10 = false; 
    let mut warning_5 = false;
    let mut shutting_down = false;

    loop {
        let capacity_string = fs::read_to_string(battery_path).expect("Failed to read battery cap");
        let status_string = fs::read_to_string(status_path).expect("Failed to read charging status");

        let capacity_int = capacity_string.trim().parse::<u8>().expect("Failed to check");
        let status  = status_string.trim();
            
            if capacity_int <= 10 && status == "Discharging" && !warning_10 && !shutting_down { // battery warning at 10%
                let _ = Command::new("/usr/bin/notify-send").arg("Battery Warning 10%").arg("Shuts down at 3%").status();
                warning_10 = true;
            } 
            if  capacity_int <= 5 && status == "Discharging" && !warning_5 && !shutting_down { // battery warning 5%
                let _ = Command::new("/usr/bin/notify-send").arg("Battery Warning 5%").arg("Shuts down at 3%\nSAVE WORK NOW").status();
                warning_5 = true;
            } 
            if status != "Discharging" { // prevents losing the warnings if you replug and let the computer drain again
                warning_10 = false;
                warning_5 = false;
            }

            // Here is where the magic happens
            if capacity_int <= 3 && status == "Discharging" && !shutting_down { 
                shutting_down = true;

                let user = std::env::var("USER").unwrap_or_else(|_| {
                    let output = Command::new("whoami").output();

                    match output {
                        Ok(output) => {
                            String::from_utf8_lossy(&output.stdout).trim().to_string()
                        },
                        Err(_) => {
                            String::new()
                        }
                    }
                });

                let _ = Command::new("/usr/bin/systemctl").arg("poweroff").status();
                // To safely log user out while shutting down to prevent hangups
                let _ = Command::new("/usr/bin/loginctl").args(["terminate-user", &user]).status();
             } 

            thread::sleep(Duration::from_secs(30)); // This is for your computer to run a check every 30 seconds to keep an eye on the battery
    } 
}
