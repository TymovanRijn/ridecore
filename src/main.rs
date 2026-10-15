mod app;

use ridecore::ble;

fn main() -> eframe::Result {
    let (event_tx, event_rx) = crossbeam_channel::unbounded();
    let commands = ble::spawn(event_tx);
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 760.0])
            .with_title("RideCore"),
        ..Default::default()
    };
    eframe::run_native(
        "RideCore",
        options,
        Box::new(|_cc| Ok(Box::new(app::RideApp::new(event_rx, commands)))),
    )
}
