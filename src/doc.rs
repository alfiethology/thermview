use crate::image::{Params, ThermalImage};
use crate::palette::{Look, Mapping, Overscan, Palette, Unit};

#[derive(Clone, Debug, PartialEq)]
pub struct Point {
    pub id: u32,
    pub x: usize,
    pub y: usize,
    pub label: String,
    pub colour: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Overlays {
    pub extremes: bool,
    pub info: bool,
    pub time: bool,
}

impl Default for Overlays {
    fn default() -> Self {
        Overlays { extremes: true, info: true, time: true }
    }
}

pub struct Doc {
    pub name: String,
    pub stamp: Option<String>,
    pub source: ThermalImage,
    pub image: ThermalImage,
    pub look: Look,
    pub unit: Unit,
    pub scale: u32,
    pub label_size: f32,
    pub overlays: Overlays,
    pub points: Vec<Point>,
    pub revision: u64,
    pub measurement: crate::measure::Measurement,
    pub calibration: crate::calibrate::Calibration,
    pub video: Option<crate::video::Video>,
    next_id: u32,
}

impl Doc {
    pub fn new(source: ThermalImage, name: &str, modified: Option<String>) -> Doc {
        let look = Look {
            palette: Palette::default(),
            min: source.stats.min.celsius,
            max: source.stats.max.celsius,
            mapping: Mapping::default(),
            overscan: Overscan::default(),
            inverted: false,
        };
        let stamp = source.camera.captured.as_deref().and_then(exif_stamp).or(modified);
        Doc {
            name: name.to_string(),
            stamp,
            image: source.clone(),
            calibration: crate::calibrate::Calibration::new(&source),
            source,
            look,
            unit: Unit::default(),
            scale: 3,
            label_size: 10.0,
            overlays: Overlays::default(),
            points: Vec::new(),
            revision: 0,
            measurement: crate::measure::Measurement::default(),
            video: None,
            next_id: 0,
        }
    }

    pub fn from_video(
        video: crate::video::Video,
        name: &str,
        modified: Option<String>,
    ) -> Result<Doc, crate::Error> {
        let mut doc = Doc::new(video.frame(0)?, name, modified);
        doc.video = Some(video);
        Ok(doc)
    }

    pub fn seek(&mut self, index: usize) -> Result<(), crate::Error> {
        let Some(video) = &self.video else { return Ok(()) };
        if index == video.current {
            return Ok(());
        }
        let source = video.frame(index)?;
        let params = self.image.params;
        let adjusted = params != self.source.params;
        let mut image = if adjusted { source.recompute(params) } else { source.clone() };
        if let Some(correction) = self.calibration.correction() {
            correction.correct(index, &source, &mut image).map_err(crate::Error::new)?;
        }
        self.image = image;
        self.source = source;
        self.points.retain(|p| p.x < self.image.width && p.y < self.image.height);
        let auto_range = video.auto_range;
        let video = self.video.as_mut().unwrap();
        video.current = index;
        video.view.reveal(index);
        self.revision += 1;
        if auto_range {
            self.auto_range();
        }
        Ok(())
    }

    pub fn params(&self) -> Params {
        self.image.params
    }

    pub fn set_params(&mut self, params: Params) {
        if params != self.image.params && self.image.recomputable() {
            self.measurement.invalidate();
            self.calibration.invalidate_report();
            self.image = self.source.recompute(params);
            if let Some(correction) = self.calibration.correction() {
                if let Err(e) =
                    correction.correct(self.current_frame(), &self.source, &mut self.image)
                {
                    self.calibration.applied = false;
                    self.calibration.error = Some(e);
                }
            }
            self.revision += 1;
        }
    }

    pub fn set_reference(&mut self, settings: crate::calibrate::Settings) {
        if settings != self.calibration.settings {
            self.calibration.settings = settings;
            self.calibration.invalidate_report();
            // Any changed reference requires an explicit new decision to apply.
            let _ = self.apply_reference(false);
        }
    }

    pub fn current_frame(&self) -> usize {
        self.video.as_ref().map_or(0, |v| v.current)
    }

    pub fn reference_scope(&self) -> Result<(usize, usize), String> {
        if self.video.is_none() || self.calibration.scope == crate::calibrate::Scope::CurrentFrame {
            let frame = self.current_frame();
            return Ok((frame, frame));
        }
        let (Some(start), Some(end)) = (self.measurement.start, self.measurement.end) else {
            return Err("Mark start and end frames in Measure first.".into());
        };
        if start > end || end >= self.video.as_ref().unwrap().len() {
            return Err("The selected Measure range is invalid. Re-mark start and end.".into());
        }
        Ok((start, end))
    }

    pub fn set_reference_scope(&mut self, scope: crate::calibrate::Scope) {
        if scope != self.calibration.scope {
            self.calibration.scope = scope;
            self.calibration.settings.fixed_reference = false;
            self.calibration.invalidate_report();
            let _ = self.apply_reference(false);
        }
    }

    pub fn apply_reference(&mut self, enabled: bool) -> Result<(), String> {
        let range = if enabled { Some(self.reference_scope()?) } else { None };
        if let Some((start, end)) = range {
            if start != end && !self.calibration.settings.fixed_reference {
                return Err("Confirm the reference stays at these pixels and at the known temperature throughout the selected range.".into());
            }
            if self.calibration.settings.reading(&self.source)?.offset_c.is_none() {
                return Err("Enter an independently known reference temperature before applying a correction.".into());
            }
        }
        let params = self.image.params;
        let mut image = if params != self.source.params {
            self.source.recompute(params)
        } else {
            self.source.clone()
        };
        if let Some((start, end)) = range {
            crate::calibrate::Correction { settings: self.calibration.settings, start, end }
                .correct(self.current_frame(), &self.source, &mut image)?;
        }
        self.image = image;
        self.calibration.applied = enabled;
        self.calibration.applied_range = range;
        if self.video.as_ref().is_none_or(|v| v.auto_range) {
            self.auto_range();
        }
        self.measurement.invalidate();
        self.revision += 1;
        Ok(())
    }

    pub fn auto_range(&mut self) {
        self.look.min = self.image.stats.min.celsius;
        self.look.max = self.image.stats.max.celsius;
    }

    pub fn add_point(&mut self, x: usize, y: usize) {
        self.next_id += 1;
        let colour = self.points.len() % crate::overlay::POINT_COLOURS.len();
        self.points.push(Point { id: self.next_id, x, y, label: self.next_id.to_string(), colour });
    }

    pub fn reading(&self, p: &Point) -> f32 {
        self.image.at(p.x, p.y).unwrap_or(f32::NAN)
    }
}

pub fn exif_stamp(s: &str) -> Option<String> {
    let s = s.trim();
    let (date, time) = s.split_once(' ').unwrap_or((s, ""));
    let date: Vec<&str> = date.split([':', '-']).collect();
    if date.len() != 3 || date.iter().any(|p| p.parse::<u32>().is_err()) {
        return Some(s.to_string());
    }
    let hm: Vec<&str> = time.split(':').take(2).collect();
    let mut out = date.join("-");
    if hm.len() == 2 {
        out += &format!(" {}:{}", hm[0], hm[1]);
    }
    Some(out)
}

pub fn file_stamp(t: std::time::SystemTime) -> Option<String> {
    let t: chrono::DateTime<chrono::Local> = t.into();
    Some(t.format("%Y-%m-%d %H:%M").to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn exif_dates_read_as_iso_minutes() {
        assert_eq!(super::exif_stamp("2017:09:08 16:04:36").as_deref(), Some("2017-09-08 16:04"));
        assert_eq!(super::exif_stamp("sometime").as_deref(), Some("sometime"));
    }
}
