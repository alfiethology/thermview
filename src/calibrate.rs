//! Independent reference-patch measurements and an optional additive correction.
//! Raw counts and file temperatures are never modified.
use crate::image::{Params, Stats, ThermalImage};
use crate::planck::Conversion;
use crate::video::Video;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub x0: usize,
    pub y0: usize,
    pub x1: usize,
    pub y1: usize,
}

impl Region {
    pub fn between(a: (usize, usize), b: (usize, usize)) -> Self {
        Self { x0: a.0.min(b.0), y0: a.1.min(b.1), x1: a.0.max(b.0), y1: a.1.max(b.1) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub region: Option<Region>,
    pub emissivity: f64,
    pub distance: f64,
    pub known_c: Option<f64>,
    pub fixed_reference: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct Reading {
    pub mean_c: f64,
    pub min_c: f64,
    pub max_c: f64,
    pub pixels: usize,
    pub offset_c: Option<f64>,
}

impl Settings {
    pub fn reading(&self, source: &ThermalImage) -> Result<Reading, String> {
        let region = self.region.ok_or("Select a reference point or area on the image.")?;
        if !self.emissivity.is_finite()
            || self.emissivity <= 0.0
            || self.emissivity > 1.0
            || !self.distance.is_finite()
            || self.distance < 0.0
            || self.known_c.is_some_and(|t| !t.is_finite() || t <= -273.15)
        {
            return Err("Reference emissivity, distance or known temperature is invalid.".into());
        }
        if region.x0 > region.x1
            || region.y0 > region.y1
            || region.x1 >= source.width
            || region.y1 >= source.height
        {
            return Err("Reference area is outside this frame. Select it again.".into());
        }
        let r = source
            .radiometry
            .as_ref()
            .ok_or("This format does not retain raw counts for reference-emissivity conversion.")?;
        let params =
            Params { emissivity: self.emissivity, distance: self.distance, ..source.params };
        let conv = Conversion::with_environment(&params, &r.planck, &r.environment);
        let mut sum = 0.0;
        let mut min = f64::INFINITY;
        let mut max = f64::NEG_INFINITY;
        let mut pixels = 0;
        for y in region.y0..=region.y1 {
            for x in region.x0..=region.x1 {
                let raw =
                    r.counts.get(y * source.width + x).ok_or("Incomplete reference raster.")?;
                let t = conv.celsius(*raw as f64);
                if !t.is_finite() || t <= -273.1 {
                    return Err(
                        "Reference area contains invalid temperatures. Select another area.".into(),
                    );
                }
                sum += t;
                min = min.min(t);
                max = max.max(t);
                pixels += 1;
            }
        }
        let mean_c = sum / pixels as f64;
        Ok(Reading {
            mean_c,
            min_c: min,
            max_c: max,
            pixels,
            offset_c: self.known_c.map(|t| t - mean_c),
        })
    }

    pub fn correct(&self, source: &ThermalImage, target: &mut ThermalImage) -> Result<f64, String> {
        let offset = self.reading(source)?.offset_c.ok_or(
            "Enter an independently known reference temperature before applying a correction.",
        )?;
        for t in &mut target.celsius {
            if t.is_finite() && *t > -273.1 {
                let corrected = *t as f64 + offset;
                *t = if corrected.is_finite() && corrected > -273.1 {
                    corrected as f32
                } else {
                    f32::NAN
                };
            }
        }
        target.stats = Stats::of(&target.celsius, target.width);
        Ok(offset)
    }
}

pub struct Sample {
    pub frame: usize,
    pub reading: Result<Reading, String>,
    pub target: Params,
}

pub struct Calibration {
    pub settings: Settings,
    pub applied: bool,
    pub selecting: bool,
    pub drag_start: Option<(usize, usize)>,
    pub samples: Vec<Sample>,
    pub error: Option<String>,
    pub notice: Option<String>,
    pub complete: bool,
    job: Option<Job>,
}

struct Job {
    next: usize,
    total: usize,
    settings: Settings,
    target: Option<Params>,
}

impl Calibration {
    pub fn new(source: &ThermalImage) -> Self {
        Self {
            settings: Settings {
                region: None,
                emissivity: 0.95,
                distance: source.params.distance,
                known_c: None,
                fixed_reference: false,
            },
            applied: false,
            selecting: false,
            drag_start: None,
            samples: Vec::new(),
            error: None,
            notice: None,
            complete: false,
            job: None,
        }
    }
    pub fn invalidate_report(&mut self) {
        self.samples.clear();
        self.job = None;
        self.complete = false;
        self.error = None;
        self.notice = None;
    }
    pub fn running(&self) -> bool {
        self.job.is_some()
    }
    pub fn progress(&self) -> Option<(usize, usize)> {
        self.job.as_ref().map(|j| (j.next, j.total))
    }
    pub fn begin(&mut self, total: usize, target: Option<Params>) {
        self.invalidate_report();
        self.job = Some(Job { next: 0, total, settings: self.settings, target });
    }
    pub fn cancel(&mut self) {
        self.job = None;
        self.complete = false;
        self.error =
            Some("Analysis cancelled. The report contains only frames checked so far.".into());
    }
    pub fn advance(&mut self, video: &Video) {
        let Some(mut job) = self.job.take() else { return };
        #[cfg(not(target_arch = "wasm32"))]
        let started = std::time::Instant::now();
        for _ in 0..4 {
            if job.next >= job.total {
                self.complete = true;
                return;
            }
            let (reading, target) = match video.frame(job.next) {
                Ok(source) => (job.settings.reading(&source), job.target.unwrap_or(source.params)),
                Err(e) => (Err(e.to_string()), job.target.unwrap_or_default()),
            };
            self.samples.push(Sample { frame: job.next, reading, target });
            job.next += 1;
            #[cfg(not(target_arch = "wasm32"))]
            if started.elapsed() >= std::time::Duration::from_millis(6) {
                break;
            }
        }
        if job.next == job.total {
            self.complete = true;
        } else {
            self.job = Some(job);
        }
    }
    pub fn csv(&self) -> String {
        let s = self.settings;
        let r = s.region.unwrap_or(Region::between((0, 0), (0, 0)));
        let mut csv = String::from(
            "version,frame_one_based,x0_zero_based,y0_zero_based,x1_zero_based,y1_zero_based,reference_emissivity,reference_distance_m,known_temperature_c,fixed_reference_confirmed,correction_applied,analysis_complete,valid_pixels,reference_mean_c,reference_min_c,reference_max_c,suggested_offset_c,target_emissivity,target_distance_m,error\n",
        );
        for sample in &self.samples {
            let (pixels, mean, min, max, offset, error) = match &sample.reading {
                Ok(v) => (
                    v.pixels.to_string(),
                    v.mean_c.to_string(),
                    v.min_c.to_string(),
                    v.max_c.to_string(),
                    v.offset_c.map(|t| t.to_string()).unwrap_or_default(),
                    String::new(),
                ),
                Err(e) => (
                    String::new(),
                    String::new(),
                    String::new(),
                    String::new(),
                    String::new(),
                    e.replace('"', "\"\""),
                ),
            };
            csv.push_str(&format!(
                "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},\"{}\"\n",
                env!("CARGO_PKG_VERSION"),
                sample.frame + 1,
                r.x0,
                r.y0,
                r.x1,
                r.y1,
                s.emissivity,
                s.distance,
                s.known_c.map(|t| t.to_string()).unwrap_or_default(),
                s.fixed_reference,
                self.applied && sample.reading.is_ok(),
                self.complete,
                pixels,
                mean,
                min,
                max,
                offset,
                if sample.reading.is_ok() {
                    sample.target.emissivity.to_string()
                } else {
                    String::new()
                },
                if sample.reading.is_ok() {
                    sample.target.distance.to_string()
                } else {
                    String::new()
                },
                error
            ));
        }
        csv
    }
}
