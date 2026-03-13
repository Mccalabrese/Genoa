use std::fs;
use std::thread;
use std::time::Duration;
use std::process::Command;

fn main() {
    let battery_path = "/sys/class/power_supply/BAT0/capacity";
    let status_path = "/sys/class/power_supply/BAT0/status";

    let mut warning_10 = false;
    let mut warning_5 = false;

    loop {
        let capacity_string = fs::read_to_string(battery_path).expect("Failed to read battery cap");
        let status_string = fs::read_to_string(status_path).expect("Failed to read charging status");

        let capacity_int = capacity_string.trim().parse::<u8>().expect("Failed to check");
        let status  = status_string.trim();
            
            if capacity_int <= 10 && status == "Discharging" && !warning_10 {
                Command::new("/usr/bin/notify-send").arg("Battery Warning 10%").arg("Shuts down at 3%").status().expect("Failed to execute");
                warning_10 = true;
            } 
            if  capacity_int <= 5 && status == "Discharging" && !warning_5 {
                Command::new("/usr/bin/notify-send").arg("Battery Warning 5%").arg("Shuts down at 3%").status().expect("Failed to execute");
                warning_5 = true;
            } 
            if status != "Discharging" {
                warning_10 = false;
                warning_5 = false;
            }

            if capacity_int <= 3 && status == "Discharging" {
                //Command::new("systemctl").arg("poweroff").status().expect("Failed to execute");
              Command::new("/usr/bin/systemctl").arg("poweroff").status().expect("Failed to execute");
             } 
            thread::sleep(Duration::from_secs(30));
    } 
}
