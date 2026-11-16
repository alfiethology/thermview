use crate::calibrate::{Region, Sample};
use crate::doc::Doc;
use egui::{Color32, Sense, Stroke, Ui, pos2, vec2};
use egui_bench::prelude::*;

pub fn show(ui: &mut Ui, doc: &mut Doc, path: Option<&std::path::Path>) {
    if !doc.source.recomputable() {
        hint(
            ui,
            "Reference calibration needs a FLIR image or video with retained raw sensor counts.",
        );
        return;
    }
    let mut settings = doc.calibration.settings;
    let unit = doc.unit;
    section(ui, "reference patch", "separate from target settings", |ui| {
        hint(
            ui,
            "Click for one pixel or drag a rectangle. Prefer an area fully inside the reference object.",
        );
        ui.horizontal(|ui| {
            if ui
                .add(
                    Key::new(if doc.calibration.selecting {
                        "cancel selection"
                    } else {
                        "select reference"
                    })
                    .outline(READOUT),
                )
                .clicked()
            {
                doc.calibration.selecting = !doc.calibration.selecting;
                doc.calibration.drag_start = None;
                if let Some(video) = &mut doc.video {
                    video.playing = false;
                }
            }
        });
        ui.horizontal(|ui| {
            if ui
                .add_enabled(settings.region.is_some(), Key::new("clear patch").outline(VALUE))
                .clicked()
            {
                settings.region = None;
                settings.fixed_reference = false;
                doc.calibration.selecting = false;
            }
        });
        if let Some(r) = settings.region {
            reading(ui, "pixels · zero-based", format!("{}, {} to {}, {}", r.x0, r.y0, r.x1, r.y1));
        }
        ui.horizontal(|ui| {
            ui.label("Reference emissivity");
            ui.add(
                egui::DragValue::new(&mut settings.emissivity)
                    .range(0.001..=1.0)
                    .speed(0.005)
                    .fixed_decimals(3),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Reference distance");
            ui.add(
                egui::DragValue::new(&mut settings.distance)
                    .range(0.0..=10000.0)
                    .clamp_existing_to_range(false)
                    .speed(0.05)
                    .suffix(" m"),
            );
        });
        hint(
            ui,
            "Reference air, humidity, reflected temperature, window and calibration coefficients come from each frame's file metadata.",
        );
        let mut known = settings.known_c.is_some();
        if ui.checkbox(&mut known, "Independently known temperature").changed() {
            settings.known_c = known.then_some(20.0);
        }
        if let Some(c) = &mut settings.known_c {
            let mut display = unit.from_celsius(*c);
            if ui.add(egui::DragValue::new(&mut display).speed(0.1).suffix(unit.symbol())).changed()
            {
                *c = unit.to_celsius(display);
            }
            hint(
                ui,
                "Enter the independently measured surface temperature, not an assumed room temperature.",
            );
        } else {
            hint(
                ui,
                "Emissivity alone supports a reference reading, not an independent temperature correction.",
            );
        }
        if doc.video.is_some() {
            ui.checkbox(&mut settings.fixed_reference, "Patch stays at these pixels and its known temperature stays constant throughout the video");
            hint(
                ui,
                "No automatic tracking. If the camera or reference moves, remove the correction and select the patch again.",
            );
        }
    });
    doc.set_reference(settings);
    ui.add_space(8.0);
    section(ui, "target", "independent emissivity", |ui| {
        let mut params = doc.params();
        ui.horizontal(|ui| {
            ui.label("Target emissivity");
            ui.add(
                egui::DragValue::new(&mut params.emissivity)
                    .range(0.001..=1.0)
                    .clamp_existing_to_range(false)
                    .speed(0.005)
                    .fixed_decimals(3),
            );
        });
        doc.set_params(params);
        hint(
            ui,
            "Applies to target readings across the image. Use Calibration at the right for other target settings. Changing target settings does not change the reference calculation.",
        );
    });
    ui.add_space(8.0);
    let current = doc.calibration.settings.reading(&doc.source);
    section(ui, "current frame", "reference before correction", |ui| {
        match &current {
            Ok(v) => {
                reading(ui, "reference mean", unit.reading(v.mean_c as f32));
                reading(
                    ui,
                    "reference range",
                    format!("{} to {}", unit.reading(v.min_c as f32), unit.reading(v.max_c as f32)),
                );
                reading(ui, "reference pixels", v.pixels.to_string());
                if let Some(offset) = v.offset_c {
                    reading(ui, "known minus measured", delta(unit, offset));
                }
            }
            Err(e) => hint(ui, e),
        }
        let mut enabled = doc.calibration.applied;
        let allowed = current.as_ref().is_ok_and(|v| v.offset_c.is_some())
            && (doc.video.is_none() || doc.calibration.settings.fixed_reference);
        if ui
            .add_enabled(
                allowed || enabled,
                egui::Checkbox::new(&mut enabled, "Apply per-frame additive correction"),
            )
            .changed()
        {
            if let Err(e) = doc.apply_reference(enabled) {
                doc.calibration.error = Some(e);
            }
        }
        if doc.calibration.applied {
            note(
                ui,
                "REFERENCE CORRECTION ACTIVE: displayed readings, measurements and image exports include the offset.",
                READOUT,
            );
        }
        hint(
            ui,
            "One reference supports an additive adjustment near its temperature. It does not establish accuracy across all temperatures or correct a target's wrong emissivity. Original counts and temperatures are retained.",
        );
    });
    ui.add_space(8.0);
    section(
        ui,
        "reference history",
        if doc.video.is_some() { "all video frames" } else { "this image" },
        |ui| {
            ui.horizontal(|ui| {
                if doc.calibration.running() {
                    if ui.add(Key::new("cancel check").outline(FAULT)).clicked() {
                        doc.calibration.cancel();
                    }
                } else if ui
                    .add_enabled(current.is_ok(), Key::new("check reference").solid(READOUT))
                    .clicked()
                {
                    if let Some(video) = &mut doc.video {
                        video.playing = false;
                        let params =
                            (doc.image.params != doc.source.params).then_some(doc.image.params);
                        doc.calibration.begin(video.len(), params);
                    } else {
                        doc.calibration.invalidate_report();
                        doc.calibration.samples.push(Sample {
                            frame: 0,
                            reading: current.clone(),
                            target: doc.params(),
                        });
                        doc.calibration.complete = true;
                    }
                    ui.ctx().request_repaint();
                }
            });
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        !doc.calibration.samples.is_empty(),
                        Key::new("export reference csv").outline(VALUE),
                    )
                    .clicked()
                {
                    let stem = doc.name.rsplit_once('.').map(|(s, _)| s).unwrap_or(&doc.name);
                    match crate::host::save(
                        path,
                        &format!("{stem}-reference.csv"),
                        doc.calibration.csv().as_bytes(),
                    ) {
                        Ok(message) => doc.calibration.notice = Some(message),
                        Err(e) => doc.calibration.error = Some(e),
                    }
                }
            });
            if let Some((done, total)) = doc.calibration.progress() {
                ui.add(
                    egui::ProgressBar::new(done as f32 / total.max(1) as f32)
                        .text(format!("{done} / {total} frames")),
                );
            }
            let valid = doc.calibration.samples.iter().filter(|s| s.reading.is_ok()).count();
            if !doc.calibration.samples.is_empty() {
                reading(
                    ui,
                    "valid / checked",
                    format!("{valid} / {}", doc.calibration.samples.len()),
                );
                if !doc.calibration.complete {
                    note(ui, "Incomplete analysis; history covers checked frames only.", FAULT);
                }
                if valid < doc.calibration.samples.len() {
                    note(ui, "Some frames failed. Gaps are not interpolated or corrected.", FAULT);
                }
                history(
                    ui,
                    &doc.calibration.samples,
                    unit,
                    doc.calibration.settings.known_c.is_some(),
                );
                egui::CollapsingHeader::new("Per-frame readings and failures").show(ui, |ui| {
                    egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                        for sample in &doc.calibration.samples {
                            match &sample.reading {
                                Ok(v) => {
                                    ui.label(format!(
                                        "Frame {}: {}{}",
                                        sample.frame + 1,
                                        unit.reading(v.mean_c as f32),
                                        v.offset_c
                                            .map(|o| format!(", offset {}", delta(unit, o)))
                                            .unwrap_or_default()
                                    ));
                                }
                                Err(e) => {
                                    ui.colored_label(
                                        FAULT,
                                        format!("Frame {}: {e}", sample.frame + 1),
                                    );
                                }
                            }
                        }
                    });
                });
            }
        },
    );
    if let Some(e) = &doc.calibration.error {
        note(ui, e, FAULT);
    }
    if let Some(message) = &doc.calibration.notice {
        note(ui, message, READOUT);
    }
}

fn delta(unit: crate::palette::Unit, c: f64) -> String {
    let value = if unit == crate::palette::Unit::Fahrenheit { c * 1.8 } else { c };
    format!("{value:+.3} {}", unit.symbol())
}

fn history(ui: &mut Ui, samples: &[Sample], unit: crate::palette::Unit, error: bool) {
    let value = |s: &Sample| {
        s.reading.as_ref().ok().map(|v| if error { -v.offset_c.unwrap_or(0.0) } else { v.mean_c })
    };
    let values: Vec<f64> = samples.iter().filter_map(value).collect();
    if values.is_empty() {
        return;
    }
    let lo = values.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let display = |c: f64| if error { delta(unit, c) } else { unit.reading(c as f32) };
    ui.label(if error {
        "Reference error (measured minus known)"
    } else {
        "Reference temperature"
    });
    ui.label(format!("{} to {}", display(lo), display(hi)));
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 100.0), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 0.0, Color32::from_gray(20));
    let plot = rect.shrink(6.0);
    let bottom = if error { lo.min(0.0) } else { lo - 0.005 };
    let top = if error { hi.max(0.0) } else { hi + 0.005 };
    let span = (top - bottom).max(0.01);
    let n = samples.last().map_or(1, |s| s.frame).max(1) as f32;
    let mut previous = None;
    for sample in samples {
        if let Some(v) = value(sample) {
            let at = pos2(
                plot.left() + sample.frame as f32 / n * plot.width(),
                plot.bottom() - ((v - bottom) / span) as f32 * plot.height(),
            );
            if let Some(from) = previous {
                p.line_segment([from, at], Stroke::new(1.5, READOUT));
            }
            p.circle_filled(at, 1.5, READOUT);
            previous = Some(at);
        } else {
            previous = None;
        }
    }
    ui.label(format!(
        "Frames 1–{} · fixed pixel coordinates",
        samples.last().map_or(1, |s| s.frame + 1)
    ));
}

pub fn region_from_drag(a: (usize, usize), b: (usize, usize)) -> Region {
    Region::between(a, b)
}
