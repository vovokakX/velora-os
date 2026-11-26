//! Stage 10 tests: virtio-net, DHCP and ping through QEMU's user network
//! (the test runner adds `-netdev user`). TCP with the host is checked by
//! the `net` mode of tests/qemu_run.py.

use crate::{kprint, net, sched, time};
use velora_net::Ipv4;

pub fn dhcp_and_ping_gateway() {
    assert!(net::present(), "no virtio-net card");
    let deadline = time::uptime_ms() + 15_000;
    let cfg = loop {
        if let Some((_, Some(c), _, _)) = net::info() {
            break c;
        }
        assert!(
            time::uptime_ms() < deadline,
            "DHCP did not answer within 15 s"
        );
        sched::sleep_ms(50);
    };
    assert_eq!(cfg.ip, Ipv4::new(10, 0, 2, 15));
    assert_eq!(cfg.gateway, Ipv4::new(10, 0, 2, 2));
    assert_eq!(cfg.netmask, Ipv4::new(255, 255, 255, 0));
    let rtt = net::ping(Ipv4::new(10, 0, 2, 2), 1, 3000).expect("no reply from the gateway");
    kprint!("(ping {} us) ", rtt);
    // Nothing answers at this address on QEMU's network.
    assert!(net::ping(Ipv4::new(10, 0, 2, 77), 2, 500).is_err());
}
