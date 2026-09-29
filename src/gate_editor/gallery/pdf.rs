//! The gallery as a file someone can keep.
//!
//! A QC record has to outlive the session that produced it, and a screenshot of
//! a scrolling grid is not a record. This writes the whole run - every specimen
//! for the selected gate, in the gallery's order - as a PDF contact sheet.
//!
//! ## Why it is written by hand
//!
//! A PDF that holds pictures and line drawings is a small format, and the two
//! things this needs are the two things it does best. A plot's PNG goes in
//! *unchanged*: its compressed rows are a `FlateDecode` image with PNG
//! predictors, so the bytes the renderer produced are the bytes in the file,
//! with no decode-and-re-encode step.
//! The outlines go in as path operators, so they stay vector-sharp at any zoom
//! and the percentages stay real text that can be searched and copied.
//!
//! Doing that through a PDF library would mean a dependency and a conversion
//! layer for the sake of a few hundred lines of very stable format. Doing it
//! directly also means the drawing comes from [`Flat`] - the same flattened
//! primitives the screen draws - so the page you export is the page you looked
//! at rather than a second drawing of the same gate.
//!
//! ## What is deliberately not carried over
//!
//! Gate fills. On screen a gate is a translucent cyan wash, which reads as
//! "this region" over a bright density plot. On paper, over a printed plot, the
//! same wash hides the events it is drawn around - and the events are the
//! evidence. The PDF strokes outlines and leaves the data visible.

use std::fmt::Write as _;
use std::sync::Arc;

use super::overlay::Flat;

/// A4 landscape, in points.
const PAGE: (f32, f32) = (842.0, 595.0);
const MARGIN: f32 = 28.0;
/// Specimens across and down. Two across keeps a plot wide enough that the
/// percentage printed on a gate is still readable on paper, which is the whole
/// reason for exporting it.
const ACROSS: usize = 2;
const DOWN: usize = 3;
pub const PER_PAGE: usize = ACROSS * DOWN;

/// One plot: the picture, and the lines to draw over it.
pub struct Drawn {
    pub name: String,
    pub png: Arc<Vec<u8>>,
    pub shapes: Vec<Flat>,
    /// The side of the square the shapes were flattened against, so they can be
    /// scaled to whatever size the page gives them.
    pub rendered_at: f32,
}

/// One specimen's row on the sheet.
pub struct Sheet {
    pub title: String,
    pub slots: Vec<Cell>,
}

/// What one slot of a specimen's row holds.
///
/// Three cases, not two. A slot with no file and a file that could not be
/// drawn used to be the same empty slot, printed "no paired file" - so a QC
/// record said a specimen had no such file when it had one that failed.
pub enum Cell {
    /// The specimen has no file for this slot.
    NoFile,
    Drawn(Drawn),
    /// The specimen has a file here, and it could not be drawn.
    Failed {
        name: String,
        reason: String,
    },
}

/// Write the whole contact sheet.
pub fn write_pdf(heading: &str, sheets: &[Sheet]) -> anyhow::Result<Vec<u8>> {
    let mut pdf = Pdf::new();

    // Reserved before the pages exist, because each page names it and it names
    // each page. Numbering it first is what lets a page dictionary be written
    // complete, rather than written with a placeholder and patched later -
    // patching would also have to search the content streams, where a sample
    // name could match the placeholder and corrupt the stream's length.
    let pages_id = pdf.reserve();

    let font = pdf.add(
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
            .to_vec(),
    );

    // Geometry of one cell, worked out once.
    let usable = (PAGE.0 - 2.0 * MARGIN, PAGE.1 - 2.0 * MARGIN - HEADER_HEIGHT);
    let cell = (usable.0 / ACROSS as f32, usable.1 / DOWN as f32);
    // Two plots side by side, the specimen's name above them and each file's
    // name below. Every one of those four bands has to come out of the cell's
    // height - leaving the file name's out was enough to drop it into the next
    // row's title.
    let plot =
        ((cell.0 - CELL_GAP * 3.0) / 2.0).min(cell.1 - TITLE_HEIGHT - NAME_HEIGHT - CELL_GAP * 2.0);
    // Height is what limits the plot, so a card is narrower than its cell.
    // Centring spreads what is left either side instead of banking it all on
    // the right, where it reads as a missing column.
    let card = plot * 2.0 + CELL_GAP;
    let indent = ((cell.0 - card) / 2.0).max(0.0);

    let mut page_ids = Vec::new();
    for (page_number, chunk) in sheets.chunks(PER_PAGE).enumerate() {
        let mut content = String::new();
        let mut images: Vec<(String, usize)> = Vec::new();

        write_text(
            &mut content,
            MARGIN,
            PAGE.1 - MARGIN - 12.0,
            12.0,
            heading,
            None,
        );
        write_text(
            &mut content,
            PAGE.0 - MARGIN,
            PAGE.1 - MARGIN - 12.0,
            9.0,
            &format!("page {}", page_number + 1),
            Some(Anchor::End),
        );

        for (at, sheet) in chunk.iter().enumerate() {
            let column = at % ACROSS;
            let row = at / ACROSS;
            let left = MARGIN + column as f32 * cell.0 + indent;
            // Rows run down the page; PDF y runs up it.
            let top = PAGE.1 - MARGIN - HEADER_HEIGHT - row as f32 * cell.1;

            write_text(
                &mut content,
                left,
                top - TITLE_HEIGHT + 3.0,
                9.5,
                &sheet.title,
                None,
            );

            for (slot, filled) in sheet.slots.iter().enumerate() {
                let x = left + slot as f32 * (plot + CELL_GAP);
                let y = top - TITLE_HEIGHT - plot;
                let drawn = match filled {
                    Cell::Drawn(drawn) => drawn,
                    Cell::NoFile => {
                        write_text(&mut content, x, y + plot / 2.0, 7.5, "no paired file", None);
                        continue;
                    }
                    Cell::Failed { name, reason } => {
                        // Framed like a plot, so it reads as a file that is
                        // there, with the reason where the plot would be.
                        let _ = writeln!(
                            content,
                            "q 0.6 w 0.85 0.1 0.1 RG {x} {y} {plot} {plot} re S Q"
                        );
                        let mut line_y = y + plot - 14.0;
                        write_text(
                            &mut content,
                            x + 4.0,
                            line_y,
                            7.5,
                            "could not be drawn:",
                            None,
                        );
                        for line in wrap(reason, FAILED_LINE_CHARS)
                            .into_iter()
                            .take(FAILED_LINES)
                        {
                            line_y -= 9.0;
                            write_text(&mut content, x + 4.0, line_y, 6.5, &line, None);
                        }
                        write_text(&mut content, x, y - NAME_HEIGHT + 3.0, 6.5, name, None);
                        continue;
                    }
                };
                let name = format!("Im{}_{}", at, slot);
                let id = pdf.add_png(&drawn.png)?;
                images.push((name.clone(), id));

                // `cm` places the unit square, so the image lands at exactly
                // this size whatever its pixel dimensions are.
                let _ = write!(content, "q {plot} 0 0 {plot} {x} {y} cm /{name} Do Q\n");
                let scale = plot / drawn.rendered_at.max(1.0);
                draw_shapes(&mut content, &drawn.shapes, x, y, plot, scale);
                let _ = write!(
                    content,
                    "q 0.6 w 0.7 0.7 0.7 RG {x} {y} {plot} {plot} re S Q\n"
                );
                write_text(
                    &mut content,
                    x,
                    y - NAME_HEIGHT + 3.0,
                    6.5,
                    &drawn.name,
                    None,
                );
            }
        }

        let stream = pdf.add_stream(content.into_bytes());
        let resources = {
            let listed = images
                .iter()
                .map(|(name, id)| format!("/{name} {id} 0 R"))
                .collect::<Vec<_>>()
                .join(" ");
            format!("<< /XObject << {listed} >> /Font << /F1 {font} 0 R >> >>")
        };
        page_ids.push(pdf.add(
            format!(
                "<< /Type /Page /Parent {pages_id} 0 R /MediaBox [0 0 {} {}] /Resources {resources} /Contents {stream} 0 R >>",
                PAGE.0, PAGE.1
            )
            .into_bytes(),
        ));
    }

    pdf.finish(pages_id, &page_ids)
}

const HEADER_HEIGHT: f32 = 26.0;
const TITLE_HEIGHT: f32 = 13.0;
/// The band under a plot holding its file name.
const NAME_HEIGHT: f32 = 11.0;
const CELL_GAP: f32 = 6.0;

enum Anchor {
    End,
}

/// How much of a failure's reason fits in the frame of a plot.
const FAILED_LINE_CHARS: usize = 44;
const FAILED_LINES: usize = 12;

/// Break `text` into lines of at most `width` characters, at spaces where it
/// can - an error message is one long line, and a PDF string does not wrap.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let mut word = word;
        loop {
            let used = line.chars().count();
            let room = if used == 0 {
                width
            } else {
                width.saturating_sub(used + 1)
            };
            let len = word.chars().count();
            if len <= room {
                if used > 0 {
                    line.push(' ');
                }
                line.push_str(word);
                break;
            }
            if used > 0 {
                lines.push(std::mem::take(&mut line));
                continue;
            }
            // A word longer than a whole line: cut it.
            let cut = word
                .char_indices()
                .nth(width)
                .map_or(word.len(), |(at, _)| at);
            lines.push(word[..cut].to_string());
            word = &word[cut..];
            if word.is_empty() {
                break;
            }
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// Flattened primitives as page operators.
///
/// `x`/`y` are the plot's bottom-left corner on the page and `scale` takes the
/// flattened pixel coordinates to points. Pixel y runs down from the plot's top
/// edge and PDF y runs up from its bottom, so every y is reflected through the
/// plot's height - the one place the two coordinate systems have to be
/// reconciled, kept in this closure so it cannot be done twice or forgotten.
pub(crate) fn draw_shapes(
    out: &mut String,
    shapes: &[Flat],
    x: f32,
    y: f32,
    side: f32,
    scale: f32,
) {
    let place = |px: f32, py: f32| (x + px * scale, y + side - py * scale);

    for shape in shapes {
        match shape {
            Flat::Path {
                points,
                closed,
                stroke,
                width,
                dashed,
                ..
            } => {
                let Some(colour) = colour_of(stroke) else {
                    continue;
                };
                let Some((first, rest)) = points.split_first() else {
                    continue;
                };
                let _ = write!(out, "q {:.3} w ", (width * scale).max(0.4));
                let _ = write!(out, "{} {} {} RG ", colour.0, colour.1, colour.2);
                if *dashed {
                    let _ = write!(out, "[3 2] 0 d ");
                }
                let start = place(first.0, first.1);
                let _ = write!(out, "{:.2} {:.2} m ", start.0, start.1);
                for point in rest {
                    let mapped = place(point.0, point.1);
                    let _ = write!(out, "{:.2} {:.2} l ", mapped.0, mapped.1);
                }
                // `s` closes the path before stroking; `S` leaves it open.
                let _ = writeln!(out, "{} Q", if *closed { "s" } else { "S" });
            }
            Flat::Ellipse {
                centre,
                radius,
                rotation,
                stroke,
                width,
                dashed,
                ..
            } => {
                let Some(colour) = colour_of(stroke) else {
                    continue;
                };
                let c = place(centre.0, centre.1);
                let (rx, ry) = (radius.0 * scale, radius.1 * scale);
                let _ = write!(out, "q {:.3} w ", (width * scale).max(0.4));
                let _ = write!(out, "{} {} {} RG ", colour.0, colour.1, colour.2);
                if *dashed {
                    let _ = write!(out, "[3 2] 0 d ");
                }
                // Rotation is applied as a transform about the centre, which is
                // what the SVG `rotate(deg cx cy)` on the same ellipse does.
                // The sign flips because the y axis does.
                let radians = -rotation.to_radians();
                let (sin, cos) = (radians.sin(), radians.cos());
                let _ = write!(
                    out,
                    "1 0 0 1 {:.2} {:.2} cm {cos:.5} {sin:.5} {:.5} {cos:.5} 0 0 cm ",
                    c.0, c.1, -sin
                );
                write_ellipse(out, rx, ry);
                let _ = writeln!(out, "S Q");
            }
            Flat::Text {
                at,
                size,
                text,
                anchor,
            } => {
                let p = place(at.0, at.1);
                let anchored = anchor.as_deref().and_then(|a| match a {
                    "end" => Some(Anchor::End),
                    _ => None,
                });
                write_text(out, p.0, p.1, (size * scale).max(4.0), text, anchored);
            }
        }
    }
}

/// An ellipse centred on the origin, as four bezier arcs.
///
/// 0.5523 is the usual circular-arc constant: the control points sit that
/// fraction of a radius along the tangent, which matches a true quarter arc to
/// within about a fifth of a percent - far below a printed line's width.
fn write_ellipse(out: &mut String, rx: f32, ry: f32) {
    const K: f32 = 0.552_284_75;
    let (kx, ky) = (rx * K, ry * K);
    let _ = write!(out, "{rx:.2} 0 m ");
    let _ = write!(out, "{rx:.2} {ky:.2} {kx:.2} {ry:.2} 0 {ry:.2} c ");
    let _ = write!(out, "{:.2} {ry:.2} {:.2} {ky:.2} {:.2} 0 c ", -kx, -rx, -rx);
    let _ = write!(
        out,
        "{:.2} {:.2} {:.2} {:.2} 0 {:.2} c ",
        -rx, -ky, -kx, -ry, -ry
    );
    let _ = write!(out, "{kx:.2} {:.2} {rx:.2} {:.2} {rx:.2} 0 c ", -ry, -ky);
}

fn write_text(out: &mut String, x: f32, y: f32, size: f32, text: &str, anchor: Option<Anchor>) {
    let text = escape(text);
    // Helvetica's average advance is near 0.5 em; close enough to right-align a
    // page number and a short label, which is all this is used for.
    let x = match anchor {
        Some(Anchor::End) => x - text.len() as f32 * size * 0.5,
        None => x,
    };
    let _ = writeln!(
        out,
        "BT /F1 {size:.2} Tf 0 0 0 rg {x:.2} {y:.2} Td ({text}) Tj ET"
    );
}

/// PDF strings end at an unbalanced parenthesis, so three characters have to be
/// escaped. A marker name with a bracket in it is not unusual.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '(' | ')' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            // The stream is assembled as UTF-8 and written out as bytes, while
            // the string is read back as WinAnsi - one byte per character. Only
            // ASCII means the same thing in both, so anything else becomes a
            // question mark rather than the two characters its UTF-8 encoding
            // would be mistaken for.
            c if !c.is_ascii() || (c as u32) < 32 => out.push('?'),
            c => out.push(c),
        }
    }
    out
}

/// The stroke colours the gate styles actually use.
///
/// `None` means "do not draw this", which is what `none` asks for.
fn colour_of(name: &str) -> Option<(f32, f32, f32)> {
    match name {
        "none" | "" => None,
        "cyan" => Some((0.0, 0.65, 0.75)),
        "orange" => Some((0.9, 0.5, 0.0)),
        "red" => Some((0.85, 0.1, 0.1)),
        "yellow" => Some((0.8, 0.7, 0.0)),
        "grey" | "gray" => Some((0.45, 0.45, 0.45)),
        // An unknown colour is drawn rather than dropped: a visible line in the
        // wrong shade beats a gate silently missing from a QC record.
        _ => Some((0.0, 0.0, 0.0)),
    }
}

// ── the container ────────────────────────────────────────────────────────────

/// Objects, in order, with a cross-reference table written at the end.
struct Pdf {
    objects: Vec<Option<Vec<u8>>>,
}

impl Pdf {
    fn new() -> Self {
        Self {
            objects: Vec::new(),
        }
    }

    fn add(&mut self, body: Vec<u8>) -> usize {
        self.objects.push(Some(body));
        self.objects.len()
    }

    /// An id for an object whose body is not known yet - a page needs to name
    /// the pages node, which cannot be numbered until every page exists.
    fn reserve(&mut self) -> usize {
        self.objects.push(None);
        self.objects.len()
    }

    fn set(&mut self, id: usize, body: Vec<u8>) {
        self.objects[id - 1] = Some(body);
    }

    fn add_stream(&mut self, data: Vec<u8>) -> usize {
        let mut body = format!("<< /Length {} >>\nstream\n", data.len()).into_bytes();
        body.extend_from_slice(&data);
        body.extend_from_slice(b"\nendstream");
        self.add(body)
    }

    /// An image object holding a PNG's pixels.
    ///
    /// A PNG's image data is a zlib stream of rows, each led by a PNG filter
    /// byte, and a PDF reads exactly that with `/FlateDecode` and PNG
    /// predictors - so the compressed bytes go in as they are, with nothing
    /// decoded or re-compressed.
    fn add_png(&mut self, png: &[u8]) -> anyhow::Result<usize> {
        let image = read_png(png)?;
        let mut body = format!(
            "<< /Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /DecodeParms << /Predictor 15 /Colors 3 /BitsPerComponent 8 /Columns {w} >> /Length {} >>\nstream\n",
            image.data.len(),
            w = image.width,
            h = image.height,
        )
        .into_bytes();
        body.extend_from_slice(&image.data);
        body.extend_from_slice(b"\nendstream");
        Ok(self.add(body))
    }

    fn finish(mut self, pages_id: usize, pages: &[usize]) -> anyhow::Result<Vec<u8>> {
        let kids = pages
            .iter()
            .map(|id| format!("{id} 0 R"))
            .collect::<Vec<_>>()
            .join(" ");
        self.set(
            pages_id,
            format!("<< /Type /Pages /Kids [{kids}] /Count {} >>", pages.len()).into_bytes(),
        );
        let catalog = self.add(format!("<< /Type /Catalog /Pages {pages_id} 0 R >>").into_bytes());

        let mut out = b"%PDF-1.4\n".to_vec();
        // A comment of high bytes, which tells anything reading the file that it
        // is binary and must not be newline-translated.
        out.extend_from_slice(b"%\xE2\xE3\xCF\xD3\n");
        let mut offsets = Vec::with_capacity(self.objects.len());
        for (at, object) in self.objects.iter().enumerate() {
            let object = object.as_ref().ok_or_else(|| {
                anyhow::anyhow!("object {} was reserved and never written", at + 1)
            })?;
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", at + 1).as_bytes());
            out.extend_from_slice(object);
            out.extend_from_slice(b"\nendobj\n");
        }

        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n", self.objects.len() + 1).as_bytes());
        out.extend_from_slice(b"0000000000 65535 f \n");
        for offset in &offsets {
            out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root {catalog} 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                self.objects.len() + 1
            )
            .as_bytes(),
        );
        Ok(out)
    }
}

/// What the PDF needs of a PNG: its size, and its image data - the
/// concatenated `IDAT` chunks.
struct PngImage {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

/// Read a PNG as far as the PDF needs it.
///
/// The size is read from the header rather than assumed from whatever size
/// was asked of the renderer: an image object has to state what the data
/// actually holds, and a wrong height is a torn or blank picture rather than
/// an error.
///
/// Only the kind of PNG the plots are drawn as is accepted - 8-bit RGB, not
/// interlaced - since the image object is declared as exactly that; anything
/// else is refused rather than embedded as a picture of noise. Chunk CRCs
/// are not checked: the bytes come from the renderer in this process.
fn read_png(png: &[u8]) -> anyhow::Result<PngImage> {
    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    let rest = png
        .strip_prefix(SIGNATURE)
        .ok_or_else(|| anyhow::anyhow!("not a PNG"))?;

    let mut header = None;
    let mut data = Vec::new();
    let mut at = 0usize;
    loop {
        let length_bytes = rest
            .get(at..at + 4)
            .ok_or_else(|| anyhow::anyhow!("PNG ends before its IEND chunk"))?;
        let length = u32::from_be_bytes(length_bytes.try_into().unwrap()) as usize;
        let kind = rest
            .get(at + 4..at + 8)
            .ok_or_else(|| anyhow::anyhow!("PNG ends inside a chunk header"))?;
        let body = at
            .checked_add(8 + length)
            .and_then(|end| rest.get(at + 8..end))
            .ok_or_else(|| anyhow::anyhow!("PNG chunk runs past the end of the file"))?;
        match kind {
            b"IHDR" => {
                if body.len() != 13 {
                    return Err(anyhow::anyhow!(
                        "PNG header is {} bytes, not 13",
                        body.len()
                    ));
                }
                let width = u32::from_be_bytes(body[0..4].try_into().unwrap());
                let height = u32::from_be_bytes(body[4..8].try_into().unwrap());
                let (depth, colour, interlace) = (body[8], body[9], body[12]);
                if depth != 8 || colour != 2 || interlace != 0 {
                    return Err(anyhow::anyhow!(
                        "PNG is not 8-bit RGB without interlacing (bit depth {depth}, colour type {colour}, interlace {interlace})"
                    ));
                }
                header = Some((width, height));
            }
            b"IDAT" => data.extend_from_slice(body),
            b"IEND" => break,
            _ => {}
        }
        // Length, type, body and CRC.
        at += 12 + length;
    }

    let (width, height) = header.ok_or_else(|| anyhow::anyhow!("PNG has no header"))?;
    if data.is_empty() {
        return Err(anyhow::anyhow!("PNG has no image data"));
    }
    Ok(PngImage {
        width,
        height,
        data,
    })
}
