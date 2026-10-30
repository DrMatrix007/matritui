#[path = "../src/pane.rs"]
mod pane;
use pane::Pane;

use std::num::NonZeroU16;
use std::time::Duration;

#[test]
fn write_input_is_read_back_and_resize_changes_parser_size() {
    let mut pane = Pane::spawn("cat", (24, 80)).unwrap();

    pane.write_input(b"hello\r\n").unwrap();
    std::thread::sleep(Duration::from_millis(200));

    {
        let parser = pane.parser().lock().unwrap();
        let contents = parser.screen().contents();
        assert!(contents.contains("hello"));
    }

    pane.resize(30, 100).unwrap();

    {
        let parser = pane.parser().lock().unwrap();
        let (rows, cols) = parser.screen().size();
        assert_eq!(rows, NonZeroU16::new(30).unwrap());
        assert_eq!(cols, NonZeroU16::new(100).unwrap());
    }

    pane.kill().unwrap();
}

#[test]
fn try_wait_exit_reports_success_for_exited_child() {
    let mut pane = Pane::spawn("true", (24, 80)).unwrap();

    let mut status = None;
    for _ in 0..100 {
        if let Some(s) = pane.try_wait_exit().unwrap() {
            status = Some(s);
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    let status = status.expect("child did not exit within the polling window");
    assert!(status.success());
}
