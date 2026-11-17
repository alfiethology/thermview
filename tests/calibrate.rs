use std::sync::Arc;
use thermview::calibrate::{Region, Settings};
use thermview::doc::Doc;
use thermview::image::{Format, Params, Radiometry, ThermalImage};
use thermview::planck::{Environment, Planck};

// Two surfaces with different emissivities at 25°C (reference) and 40°C
// (target), against reflected radiation at 20°C. Counts include quantization.
fn source() -> ThermalImage {
    let p = Planck::default();
    let radiance = |t: f64| p.r1 / (p.r2 * ((p.b / (t + 273.15)).exp() - p.f)) - p.o;
    let count = |e: f64, t: f64| (e * radiance(t) + (1.0 - e) * radiance(20.0)).round() as u16;
    let counts = vec![count(0.97, 25.0), count(0.97, 25.0), count(0.8, 40.0), count(0.8, 40.0)];
    let mut source = ThermalImage::new(Format::FlirJpeg, 2, 2, vec![0.0; 4]);
    source.params = Params { emissivity: 0.8, distance: 0.0, ..Params::default() };
    source.radiometry =
        Some(Arc::new(Radiometry { counts, planck: p, environment: Environment::default() }));
    source.recompute(source.params)
}

fn settings() -> Settings {
    Settings {
        region: Some(Region::between((1, 0), (0, 0))),
        emissivity: 0.97,
        distance: 0.0,
        known_c: Some(25.0),
        fixed_reference: false,
    }
}

#[test]
fn separate_emissivities_recover_known_surfaces_without_changing_target_parameters() {
    let source = source();
    let reading = settings().reading(&source).unwrap();
    assert!((reading.mean_c - 25.0).abs() < 0.02);
    assert!((source.at(0, 1).unwrap() - 40.0).abs() < 0.02);
    assert!((source.at(0, 0).unwrap() as f64 - reading.mean_c).abs() > 0.5);
    assert_eq!(reading.pixels, 2);
    assert_eq!(source.params.emissivity, 0.8);
}

#[test]
fn correction_is_reversible_not_cumulative_and_reference_ignores_target_edits() {
    let mut doc = Doc::new(source(), "test.jpg", None);
    let original = doc.image.celsius.clone();
    let counts = doc.source.radiometry.as_ref().unwrap().counts.clone();
    let mut reference = settings();
    reference.known_c = Some(reference.reading(&doc.source).unwrap().mean_c + 2.0);
    doc.set_reference(reference);
    doc.apply_reference(true).unwrap();
    assert!((doc.image.at(0, 1).unwrap() - original[2] - 2.0).abs() < 1e-5);
    doc.apply_reference(true).unwrap();
    assert!((doc.image.at(0, 1).unwrap() - original[2] - 2.0).abs() < 1e-5);
    let before = reference.reading(&doc.source).unwrap().mean_c;
    doc.set_params(Params { emissivity: 0.7, ..doc.params() });
    assert_eq!(reference.reading(&doc.source).unwrap().mean_c, before);
    assert_eq!(doc.params().emissivity, 0.7);
    assert_eq!(doc.calibration.settings.emissivity, 0.97);
    doc.apply_reference(false).unwrap();
    assert_eq!(doc.image.celsius, doc.source.recompute(doc.params()).celsius);
    assert_eq!(doc.source.celsius, original);
    assert_eq!(doc.source.radiometry.as_ref().unwrap().counts, counts);
    doc.set_params(doc.source.params);
    assert_eq!(doc.image.celsius, original);
}

#[test]
fn missing_temperature_invalid_reference_or_finished_temperature_plane_cannot_correct() {
    let source = source();
    let mut s = settings();
    s.known_c = None;
    assert!(s.reading(&source).unwrap().offset_c.is_none());
    assert!(s.correct(&source, &mut source.clone()).is_err());
    for value in [0.0, -0.1, 1.1, f64::NAN] {
        s = settings();
        s.emissivity = value;
        assert!(s.reading(&source).is_err());
    }
    s = settings();
    s.region = Some(Region::between((2, 0), (2, 1)));
    assert!(s.reading(&source).is_err());
    let image = ThermalImage::new(Format::Hikmicro, 2, 2, vec![25.0; 4]);
    assert!(settings().reading(&image).is_err());
    let mut invalid = source.clone();
    let mut r = invalid.radiometry.as_ref().unwrap().as_ref().clone();
    r.counts[0] = 0;
    invalid.radiometry = Some(Arc::new(r));
    assert!(settings().reading(&invalid).is_err());
}

#[test]
fn editing_reference_removes_correction_and_invalidates_history() {
    let mut doc = Doc::new(source(), "test.jpg", None);
    doc.set_reference(settings());
    doc.apply_reference(true).unwrap();
    doc.calibration.begin(10, None);
    doc.set_reference(Settings { emissivity: 0.96, ..settings() });
    assert!(!doc.calibration.applied);
    assert!(!doc.calibration.running());
    assert_eq!(doc.image.celsius, doc.source.celsius);
}

#[test]
fn calibrate_tab_displays_separate_settings_history_and_correction_state() {
    let mut h = thermview::headless::Harness::new(1600.0, 1200.0);
    let mut image = source();
    // A larger synthetic view makes the screenshot and drag interaction readable.
    let counts = (0..80 * 60).map(|i| 17000 + ((i % 80) * 45) as u16).collect();
    let radiometry = image.radiometry.as_ref().unwrap();
    image.width = 80;
    image.height = 60;
    image.radiometry = Some(Arc::new(Radiometry {
        counts,
        planck: radiometry.planck,
        environment: radiometry.environment,
    }));
    image = image.recompute(image.params);
    let mut doc = Doc::new(image, "reference.jpg", None);
    doc.scale = 8;
    let mut reference = settings();
    reference.region = Some(Region::between((5, 5), (12, 12)));
    reference.known_c = Some(25.0);
    doc.set_reference(reference);
    h.app.doc = Some(doc);
    h.settle();
    h.click(h.find("CALIBRATE").unwrap().center());
    assert!(h.find("Reference emissivity").is_some());
    assert!(h.find("Target emissivity").is_some());
    h.click(h.find("CHECK REFERENCE").unwrap().center());
    assert_eq!(h.app.doc.as_ref().unwrap().calibration.samples.len(), 1);
    assert!(h.app.doc.as_ref().unwrap().calibration.csv().contains("suggested_offset_c"));
    std::fs::write(std::env::temp_dir().join("thermview-calibrate-preview.png"), h.png()).unwrap();
    let anchor = h.thermal_rect().unwrap().center();
    h.frame(vec![
        egui::Event::PointerMoved(anchor),
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, 400.0),
            modifiers: egui::Modifiers::CTRL,
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
        },
    ]);
    h.settle();
    assert!(h.app.doc.as_ref().unwrap().view.zoom > 2.0);
    h.click(h.find("SELECT REFERENCE").unwrap().center());
    let rect = h.thermal_rect().expect("thermal image bounds");
    let pixel = |x: usize, y: usize| {
        rect.min
            + egui::vec2(
                (x as f32 + 0.5) * rect.width() / 80.0,
                (y as f32 + 0.5) * rect.height() / 60.0,
            )
    };
    h.click(pixel(35, 25));
    let doc = h.app.doc.as_ref().unwrap();
    assert_eq!(doc.calibration.settings.region, Some(Region::between((35, 25), (35, 25))));
    assert!(doc.points.is_empty(), "reference selection must not create a target spot");
    h.click(h.find("SELECT REFERENCE").unwrap().center());
    let view = h.app.doc.as_ref().unwrap().view;
    h.drag(pixel(45, 35), pixel(38, 28));
    assert_eq!(
        h.app.doc.as_ref().unwrap().view,
        view,
        "selecting a reference must not pan the zoomed image"
    );
    let doc = h.app.doc.as_ref().unwrap();
    assert_eq!(doc.calibration.settings.region, Some(Region::between((38, 28), (45, 35))));
    assert!(!doc.calibration.selecting);
    assert!(doc.points.is_empty());
    h.app.doc.as_mut().unwrap().next_hotspot();
    assert!(h.app.doc.as_ref().unwrap().hotspots.count().is_some());
    h.app.doc.as_mut().unwrap().apply_reference(true).unwrap();
    assert!(
        h.app.doc.as_ref().unwrap().hotspots.count().is_none(),
        "correction must clear cached uncorrected hot spots"
    );
    h.settle();
    assert!(h.texts().iter().any(|(text, _)| text.starts_with("REFERENCE OFFSET")));
    std::fs::write(std::env::temp_dir().join("thermview-calibrate-active-preview.png"), h.png())
        .unwrap();
}
