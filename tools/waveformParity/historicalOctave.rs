use dekzer_playback::waveform_octave::OctaveAnalyzer;
use serde_json::{Value, json};

pub struct Work {
    analyzer: Option<OctaveAnalyzer>,
    powers: Vec<f64>,
}

impl Work {
    pub fn new(rate: u32, before: u64, start: u64, end: u64, enabled: bool) -> Result<Self, String> {
        let analyzer = enabled.then(|| OctaveAnalyzer::new(rate, before, start, end)).transpose()
            .map_err(|e| e.to_string())?;
        Ok(Self { analyzer, powers: vec![] })
    }

    pub fn push(&mut self, frames: &[[f32; 2]]) -> Result<(), String> {
        if let Some(analyzer) = &mut self.analyzer {
            analyzer.push(frames, |cell| self.powers.extend(cell.energy)).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub fn finish(mut self, start: u64, end: u64) -> Result<Option<Value>, String> {
        let Some(analyzer) = &mut self.analyzer else { return Ok(None) };
        analyzer.finish(|cell| self.powers.extend(cell.energy)).map_err(|e| e.to_string())?;
        Ok(Some(json!({
            "version": 1, "startFrame": start, "frameCount": end - start,
            "firstCellFrame": start / 256 * 256, "framesPerColumn": 256, "powers": self.powers,
        })))
    }
}
