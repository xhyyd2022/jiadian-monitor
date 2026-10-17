//! Probe v9: write command, wait longer, read back. Try known battery
//! command patterns for 2.4G ODM mice.

use hidapi::HidApi;

fn try_cmd(dev: &hidapi::HidDevice, name: &str, cmd: &[u8]) {
    // Reset: write zeros first
    let mut reset = vec![0u8; 32];
    reset[0] = 0x05;
    let _ = dev.send_feature_report(&reset);
    std::thread::sleep(std::time::Duration::from_millis(50));

    print!("{:30} -> ", name);
    match dev.send_feature_report(cmd) {
        Ok(_) => {}
        Err(e) => { println!("set err: {}", e); return; }
    }
    std::thread::sleep(std::time::Duration::from_millis(150));
    let mut rbuf = vec![0u8; 32];
    rbuf[0] = 0x05;
    match dev.get_feature_report(&mut rbuf) {
        Ok(n) => {
            for i in 0..n.min(16) { print!("{:02x} ", rbuf[i]); }
            println!();
        }
        Err(e) => println!("get err: {}", e),
    }
}

fn main() {
    let api = HidApi::new().expect("HidApi");
    let mut target = None;
    for info in api.device_list() {
        if info.vendor_id() == 0x1915 && info.product_id() == 0xae8a
            && info.usage_page() == 0xffa0 && info.usage() == 0x0001 {
            target = Some(info.path().to_owned());
            break;
        }
    }
    let path = target.expect("not found");
    let dev = api.open_path(&path).expect("open failed");

    // Try various command bytes at position 1
    for b in 0x00u8..=0x1F {
        let mut cmd = vec![0u8; 32];
        cmd[0] = 0x05;
        cmd[1] = b;
        try_cmd(&dev, &format!("cmd byte1={:02x}", b), &cmd);
    }
}
