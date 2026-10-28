//! Reading the text the user painted over, with the OCR engine built into
//! Windows: offline, and nothing to bundle.

use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;

/// Pixels in BGRA rows, top to bottom.
pub struct Image {
    pub width: i32,
    pub height: i32,
    pub pixels: Vec<u8>,
}

/// What the user painted on a screenshot: the brush's coverage, one byte per
/// pixel, and the box around it.
pub struct Painted {
    pub shot: Image,
    pub mask: Vec<u8>,
    /// left, top, right, bottom (exclusive).
    pub bounds: [i32; 4],
    pub brush: i32,
}

pub enum EngineError {
    /// Windows has no recognizer for this language's script; one can be installed.
    NotInstalled(String),
    Other(String),
}

pub struct Engine {
    pub ocr: OcrEngine,
    /// The tradu language asked for ("" = Windows' choice).
    pub lang: String,
    /// Whether it's that language's own recognizer, not one for another
    /// language with the same script (English reading German, say).
    pub exact: bool,
}

/// The recognizer for `lang` (a tradu language code such as "de" or "zh-TW").
/// Without one, any installed recognizer for the same script stands in: they
/// read each other's letters well. With an empty `lang`, the first of the
/// user's Windows languages that has one.
pub fn engine(lang: &str) -> Result<Engine, EngineError> {
    let other = |e: windows::core::Error| EngineError::Other(format!("Text recognition failed: {}", e.message()));
    if lang.is_empty() {
        let ocr = OcrEngine::TryCreateFromUserProfileLanguages()
            .map_err(|_| EngineError::Other("Windows has no text recognition for your languages: :ocr de picks one".into()))?;
        return Ok(Engine { ocr, lang: String::new(), exact: true });
    }
    let wanted = lang.to_ascii_lowercase();
    let mut stand_in = None;
    for available in OcrEngine::AvailableRecognizerLanguages().map_err(other)? {
        let tag = available.LanguageTag().map_err(other)?.to_string().to_ascii_lowercase();
        if same_language(&wanted, &tag) {
            let ocr = OcrEngine::TryCreateFromLanguage(&available).map_err(other)?;
            return Ok(Engine { ocr, lang: lang.into(), exact: true });
        }
        // English first: its recognizer is the most general.
        if script(&tag) == script(&wanted) && (stand_in.is_none() || tag.starts_with("en")) {
            stand_in = Some(available);
        }
    }
    match stand_in {
        Some(available) => {
            let ocr = OcrEngine::TryCreateFromLanguage(&available).map_err(other)?;
            Ok(Engine { ocr, lang: lang.into(), exact: false })
        }
        None => Err(EngineError::NotInstalled(lang.into())),
    }
}

/// The writing system of a language code or tag, roughly.
fn script(tag: &str) -> &'static str {
    if tag.contains("-latn") {
        return "Latn";
    }
    if tag.contains("-cyrl") {
        return "Cyrl";
    }
    match tag.split('-').next().unwrap_or("") {
        "ru" | "uk" | "bg" | "be" | "mk" | "sr" | "kk" | "ky" | "mn" | "tg" => "Cyrl",
        "el" => "Grek",
        "ar" | "fa" | "ur" | "ps" | "sd" | "ug" => "Arab",
        "he" | "iw" | "yi" => "Hebr",
        "hi" | "mr" | "ne" | "sa" => "Deva",
        "ja" => "Jpan",
        "ko" => "Kore",
        "zh" if tag.contains("hant") || tag.ends_with("-tw") || tag.ends_with("-hk") => "Hant",
        "zh" => "Hans",
        "th" => "Thai",
        "ka" => "Geor",
        "hy" => "Armn",
        "bn" | "as" => "Beng",
        "ta" => "Taml",
        "te" => "Telu",
        "km" => "Khmr",
        "am" => "Ethi",
        _ => "Latn",
    }
}

/// "de" matches "de-DE"; Chinese also has to match the script.
fn same_language(wanted: &str, tag: &str) -> bool {
    let primary = |t: &str| t.split('-').next().unwrap_or("").to_owned();
    if primary(wanted) != primary(tag) {
        return false;
    }
    if primary(wanted) != "zh" {
        return true;
    }
    let traditional = |t: &str| t.contains("hant") || t.ends_with("-tw") || t.ends_with("-hk");
    traditional(wanted) == traditional(tag)
}

/// The PowerShell line (run as admin) that installs the recognizer for `lang`.
pub fn install_command(lang: &str) -> String {
    let tag = match lang {
        "en" => "en-US", "de" => "de-DE", "fr" => "fr-FR", "es" => "es-ES", "it" => "it-IT",
        "pt" => "pt-BR", "ja" => "ja-JP", "ko" => "ko-KR", "ru" => "ru-RU", "pl" => "pl-PL",
        "nl" => "nl-NL", "sv" => "sv-SE", "da" => "da-DK", "fi" => "fi-FI", "no" | "nb" => "nb-NO",
        "tr" => "tr-TR", "cs" => "cs-CZ", "el" => "el-GR", "hu" => "hu-HU", "ro" => "ro-RO",
        "ar" => "ar-SA", "zh-CN" => "zh-CN", "zh-TW" => "zh-TW", "sk" => "sk-SK", "hr" => "hr-HR",
        "sr" => "sr-Latn-RS", "bg" => "bg-BG", "sl" => "sl-SI", "uk" => "uk-UA",
        other => return format!("Get-WindowsCapability -Online -Name 'Language.OCR*{other}*' | Add-WindowsCapability -Online"),
    };
    format!("Add-WindowsCapability -Online -Name \"Language.OCR~~~{tag}~0.0.1.0\"")
}

struct Word {
    text: String,
    /// x, y, width, height in screen pixels.
    rect: [f32; 4],
}

/// The painted-over words, in reading order. Words the brush only grazed are
/// left out; lines that are far apart become separate paragraphs.
pub fn read(engine: &Engine, painted: &Painted) -> windows::core::Result<String> {
    let tag = engine.ocr.RecognizerLanguage()?.LanguageTag()?.to_string();
    // Chinese and Japanese are recognized one character per "word".
    let gap = if tag.starts_with("ja") || tag.starts_with("zh") { "" } else { " " };

    let [left, top, right, bottom] = painted.bounds;
    // Room for the rest of a word the brush started or ended in the middle
    // of; German compounds run long.
    let pad_x = (painted.brush * 16).max(200);
    let pad_y = painted.brush * 2;
    let crop = [
        (left - pad_x).max(0),
        (top - pad_y).max(0),
        (right + pad_x).min(painted.shot.width),
        (bottom + pad_y).min(painted.shot.height),
    ];

    // Game text is small and busy; first as it is, then cleaned up.
    let mut lines = Vec::new();
    for clean in [false, true] {
        lines = recognize_lines(&engine.ocr, painted, crop, clean)?;
        if !lines.is_empty() {
            break;
        }
    }
    let text = join_lines(&lines, gap);
    // Other Latin recognizers read German's ß as B ("weiB", "GröBe").
    if engine.lang.starts_with("de") && !engine.exact {
        return Ok(text.split(' ').map(eszett).collect::<Vec<_>>().join(" "));
    }
    Ok(text)
}

/// "weiB" → "weiß": a capital B after the first letter of a lowercase word.
fn eszett(word: &str) -> String {
    let letters: Vec<char> = word.chars().collect();
    let lowercase_rest = letters.iter().skip(1).filter(|c| c.is_alphabetic() && **c != 'B').all(|c| c.is_lowercase());
    if letters.len() < 3 || !lowercase_rest {
        return word.into();
    }
    letters.iter().enumerate().map(|(i, &c)| if i > 0 && c == 'B' { 'ß' } else { c }).collect()
}

fn recognize_lines(
    engine: &OcrEngine,
    painted: &Painted,
    crop: [i32; 4],
    clean: bool,
) -> windows::core::Result<Vec<Vec<Word>>> {
    let (w, h) = (crop[2] - crop[0], crop[3] - crop[1]);
    let max = OcrEngine::MaxImageDimension()? as i32;
    // Windows OCR reads large text best: enlarge small crops.
    // (3x reads worse than 2x or 4x in tests: words come apart.)
    let mut scale = if h <= 120 { 4 } else if h <= 600 { 2 } else { 1 };
    while scale > 1 && (w * scale > max || h * scale > max) {
        scale -= 1;
    }
    let image = prepare(&painted.shot, crop, scale, clean);

    let writer = DataWriter::new()?;
    writer.WriteBytes(&image.pixels)?;
    let bitmap = SoftwareBitmap::CreateCopyFromBuffer(
        &writer.DetachBuffer()?,
        BitmapPixelFormat::Bgra8,
        image.width,
        image.height,
    )?;
    let result = engine.RecognizeAsync(&bitmap)?.join()?;

    let mut lines = Vec::new();
    for line in result.Lines()? {
        let mut words = Vec::new();
        for word in line.Words()? {
            let r = word.BoundingRect()?;
            let s = scale as f32;
            let rect = [crop[0] as f32 + r.X / s, crop[1] as f32 + r.Y / s, r.Width / s, r.Height / s];
            words.push(Word { text: word.Text()?.to_string(), rect });
        }
        let words: Vec<Word> = rejoin_split_words(words).into_iter().filter(|w| painted_over(painted, w.rect)).collect();
        if !words.is_empty() {
            lines.push(words);
        }
    }
    Ok(lines)
}

/// Windows sometimes splits a word in two ("dam age"): the gap between the
/// halves is much narrower than the line's real spaces.
fn rejoin_split_words(words: Vec<Word>) -> Vec<Word> {
    let gap = |a: &Word, b: &Word| b.rect[0] - (a.rect[0] + a.rect[2]);
    let mut gaps: Vec<f32> = words.windows(2).map(|w| gap(&w[0], &w[1])).collect();
    // With fewer gaps there's no telling a narrow space from a split.
    if gaps.len() < 3 {
        return words;
    }
    gaps.sort_by(f32::total_cmp);
    let usual = gaps[gaps.len() / 2];
    let mut out: Vec<Word> = Vec::with_capacity(words.len());
    for word in words {
        let letters = |t: &str| t.chars().all(char::is_alphabetic);
        match out.last_mut() {
            Some(last) if gap(last, &word) < usual * 0.6 && letters(&last.text) && letters(&word.text) => {
                last.text.push_str(&word.text);
                let right = word.rect[0] + word.rect[2];
                let top = last.rect[1].min(word.rect[1]);
                let bottom = (last.rect[1] + last.rect[3]).max(word.rect[1] + word.rect[3]);
                last.rect = [last.rect[0], top, right - last.rect[0], bottom - top];
            }
            _ => out.push(word),
        }
    }
    out
}

/// Whether the brush covered a fair part of the word, or its middle.
fn painted_over(painted: &Painted, [x, y, w, h]: [f32; 4]) -> bool {
    let at = |px: f32, py: f32| {
        let (px, py) = (px as i32, py as i32);
        px >= 0
            && py >= 0
            && px < painted.shot.width
            && py < painted.shot.height
            && painted.mask[(py * painted.shot.width + px) as usize] != 0
    };
    if at(x + w / 2.0, y + h / 2.0) {
        return true;
    }
    const STEPS: i32 = 8;
    let mut hits = 0;
    for i in 0..STEPS {
        for j in 0..STEPS {
            let px = x + w * (i as f32 + 0.5) / STEPS as f32;
            let py = y + h * (j as f32 + 0.5) / STEPS as f32;
            hits += at(px, py) as i32;
        }
    }
    hits * 10 >= STEPS * STEPS * 3
}

/// Words joined into lines, and lines into text: wrapped lines with a space,
/// lines far apart with a line break, a word hyphenated across lines rejoined.
fn join_lines(lines: &[Vec<Word>], gap: &str) -> String {
    let mut text = String::new();
    let mut previous: Option<[f32; 4]> = None;
    for words in lines {
        let line: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
        let line = line.join(gap);
        let top = words.iter().map(|w| w.rect[1]).fold(f32::MAX, f32::min);
        let bottom = words.iter().map(|w| w.rect[1] + w.rect[3]).fold(0.0, f32::max);
        if let Some([_, prev_top, _, prev_bottom]) = previous {
            let height = (prev_bottom - prev_top).max(bottom - top);
            if top - prev_bottom > height * 1.5 {
                text.push('\n');
            } else if text.ends_with('-') && line.starts_with(|c: char| c.is_lowercase()) {
                text.pop();
            } else {
                text.push_str(gap);
            }
        }
        text.push_str(&line);
        previous = Some([0.0, top, 0.0, bottom]);
    }
    text
}

/// The crop, enlarged `scale` times. `clean` makes it grayscale with dark text
/// on light and the contrast stretched, which helps with outlined or glowing text.
fn prepare(shot: &Image, [x0, y0, x1, y1]: [i32; 4], scale: i32, clean: bool) -> Image {
    let (w, h) = (x1 - x0, y1 - y0);
    let (out_w, out_h) = (w * scale, h * scale);
    let luma = |x: i32, y: i32| {
        let i = ((y * shot.width + x) * 4) as usize;
        let p = &shot.pixels[i..i + 3];
        (p[0] as u32 * 29 + p[1] as u32 * 150 + p[2] as u32 * 77) >> 8
    };

    let (mut low, mut high, mut sum) = (255u32, 0u32, 0u64);
    if clean {
        for y in y0..y1 {
            for x in x0..x1 {
                let l = luma(x, y);
                low = low.min(l);
                high = high.max(l);
                sum += l as u64;
            }
        }
    }
    // Mostly dark means light text on a dark background: flip it.
    let invert = clean && sum < (w as u64 * h as u64) * 128;
    let range = (high.saturating_sub(low)).max(1);

    let mut pixels = vec![0u8; (out_w * out_h * 4) as usize];
    for oy in 0..out_h {
        // Bilinear: sample between source pixels.
        let sy = ((oy as f32 + 0.5) / scale as f32 - 0.5).clamp(0.0, (h - 1) as f32);
        let (ya, fy) = (sy as i32, sy.fract());
        let yb = (ya + 1).min(h - 1);
        for ox in 0..out_w {
            let sx = ((ox as f32 + 0.5) / scale as f32 - 0.5).clamp(0.0, (w - 1) as f32);
            let (xa, fx) = (sx as i32, sx.fract());
            let xb = (xa + 1).min(w - 1);
            let o = ((oy * out_w + ox) * 4) as usize;
            for c in 0..3 {
                let at = |x: i32, y: i32| {
                    if clean {
                        let v = (luma(x0 + x, y0 + y).saturating_sub(low) * 255 / range) as f32;
                        if invert { 255.0 - v } else { v }
                    } else {
                        shot.pixels[(((y0 + y) * shot.width + x0 + x) * 4) as usize + c] as f32
                    }
                };
                let top = at(xa, ya) * (1.0 - fx) + at(xb, ya) * fx;
                let bottom = at(xa, yb) * (1.0 - fx) + at(xb, yb) * fx;
                pixels[o + c] = (top * (1.0 - fy) + bottom * fy) as u8;
            }
            pixels[o + 3] = 255;
        }
    }
    Image { width: out_w, height: out_h, pixels }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use windows::core::w;
    use windows::Win32::Foundation::{COLORREF, RECT};
    use windows::Win32::Graphics::Gdi::*;

    /// Light text with a dark outline on a dark, noisy background, like a game tooltip.
    pub fn render(lines: &[&str], size: i32) -> Image {
        let (width, height) = (900, 40 + lines.len() as i32 * size * 3 / 2);
        unsafe {
            let dc = CreateCompatibleDC(None);
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bitmap = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0).unwrap();
            SelectObject(dc, bitmap.into());
            let pixels = std::slice::from_raw_parts_mut(bits as *mut u8, (width * height * 4) as usize);
            for (i, p) in pixels.chunks_exact_mut(4).enumerate() {
                let n = ((i * 2654435761) >> 7) as u8 % 24;
                p.copy_from_slice(&[30 + n, 24 + n, 18 + n, 255]);
            }
            let font = CreateFontW(-size, 0, 0, 0, 600, 0, 0, 0, DEFAULT_CHARSET, OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS, ANTIALIASED_QUALITY, 0, w!("Segoe UI"));
            SelectObject(dc, font.into());
            SetBkMode(dc, TRANSPARENT);
            for (i, line) in lines.iter().enumerate() {
                let text: Vec<u16> = line.encode_utf16().collect();
                let y = 20 + i as i32 * size * 3 / 2;
                SetTextColor(dc, COLORREF(0x000000));
                for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                    let _ = TextOutW(dc, 20 + dx, y + dy, &text);
                }
                SetTextColor(dc, COLORREF(0x00b0e8f0));
                let _ = TextOutW(dc, 20, y, &text);
            }
            let _ = GdiFlush();
            let image = Image { width, height, pixels: pixels.to_vec() };
            let _ = DeleteObject(font.into());
            let _ = DeleteObject(bitmap.into());
            let _ = DeleteDC(dc);
            image
        }
    }

    fn painted(shot: Image, rects: &[RECT], brush: i32) -> Painted {
        let mut mask = vec![0u8; (shot.width * shot.height) as usize];
        let mut bounds = [i32::MAX, i32::MAX, 0, 0];
        for r in rects {
            for y in r.top..r.bottom {
                for x in r.left..r.right {
                    mask[(y * shot.width + x) as usize] = 1;
                }
            }
            bounds = [bounds[0].min(r.left), bounds[1].min(r.top), bounds[2].max(r.right), bounds[3].max(r.bottom)];
        }
        Painted { shot, mask, bounds, brush }
    }

    fn engine() -> Engine {
        super::engine("en").ok().or_else(|| super::engine("").ok()).expect("an OCR language")
    }

    #[test]
    fn restores_eszett() {
        assert_eq!(eszett("weiB"), "weiß");
        assert_eq!(eszett("GröBe"), "Größe");
        assert_eq!(eszett("BMW"), "BMW");
        assert_eq!(eszett("Bau"), "Bau");
        assert_eq!(eszett("FuBball"), "Fußball");
    }

    #[test]
    fn reads_german_without_its_recognizer() {
        let size = 15;
        let shot = render(&["Erhöht die Größe um zwölf Prozent"], size);
        let p = painted(shot, &[RECT { left: 20, top: 22, right: 340, bottom: 34 }], 8);
        let Ok(engine) = super::engine("de") else { return };
        let text = read(&engine, &p).unwrap();
        println!("{text:?} exact={}", engine.exact);
        assert_eq!(text, "Erhöht die Größe um zwölf Prozent");
    }

    #[test]
    fn reads_only_the_painted_line() {
        let size = 15;
        let shot = render(&["Increases attack damage by 20", "Grants bonus movement speed"], size);
        // A brush stroke along the second line only.
        let y = 20 + size * 3 / 2 + size / 2;
        let p = painted(shot, &[RECT { left: 20, top: y - 8, right: 300, bottom: y + 8 }], 8);
        let text = read(&engine(), &p).unwrap();
        println!("{text:?}");
        assert_eq!(text, "Grants bonus movement speed");
    }

    #[test]
    fn joins_wrapped_lines() {
        let size = 15;
        let shot = render(&["Your next basic attack deals", "bonus magic damage"], size);
        let p = painted(shot, &[RECT { left: 20, top: 20, right: 320, bottom: 20 + size * 5 / 2 + 2 }], 8);
        let text = read(&engine(), &p).unwrap();
        println!("{text:?}");
        assert_eq!(text, "Your next basic attack deals bonus magic damage");
    }

    #[test]
    fn reads_one_word() {
        let size = 14;
        let shot = render(&["Armor penetration and lethality"], size);
        // Just over "penetration", roughly.
        let p = painted(shot, &[RECT { left: 80, top: 24, right: 140, bottom: 32 }], 7);
        let text = read(&engine(), &p).unwrap();
        println!("{text:?}");
        assert_eq!(text, "penetration");
    }
}
