//! World layers (.lrent): the placed entities, their transforms, names and bounding boxes.

use anyhow::{anyhow, bail, Result};

const MARK_END_OBJECT: [u8; 4] = [0xde, 0xc0, 0xad, 0xde];
const MARK_END_CONTENT: [u8; 4] = [0xef, 0xbe, 0xad, 0xde];
const RECORD_VERSION: [u8; 2] = [0xd6, 0x00];
const COUNT_OFFSET: usize = 194;
const ROOT_ENTITY_OFFSET: usize = 198;
const FIRST_ENTITY_OFFSET: usize = 502;
const CONTEXT_BOX_OFFSETS: [usize; 2] = [143, 170];

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlacedEntity {
    pub index: u32,
    pub parent: u32,
    pub name: String,
    pub matrix: [f32; 16],
    pub position: [f32; 3],
    pub size: [f32; 3],
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Copy)]
struct Fields {
    world_matrix: usize,
    local_matrix: Option<usize>,
    bbox: Option<usize>,
    centre: Option<usize>,
    bbox2: Option<usize>,
    centre2: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct LrentDoc {
    data: Vec<u8>,
    content_start: usize,
    pool_start: usize,
    strings: Vec<String>,
    starts: Vec<usize>,
    table_start: usize,
    pairs: Vec<(u32, u32)>,
    container_sizes: Vec<usize>,
}

const LOCAL_BOX_OFFSET: usize = 128;

#[derive(Debug, Clone, Default, serde::Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GeometryAudit {
    pub entities: usize,
    pub no_fields: usize,
    pub no_box: usize,
    pub no_local_box: usize,
    pub with_boxes: usize,
    pub box_is_transformed_local_box: usize,
    pub radius_is_half_diagonal: usize,
    pub centre_is_box_centre: usize,
    pub has_second_block: usize,
    pub second_block_agrees: usize,
    pub roots: usize,
    pub root_local_equals_world: usize,
    pub layers: usize,
    pub table_children_complete: usize,
    pub table_roots_first: usize,
    pub examples: Vec<String>,
}

impl GeometryAudit {
    pub fn merge(&mut self, other: GeometryAudit) {
        self.entities += other.entities;
        self.no_fields += other.no_fields;
        self.no_box += other.no_box;
        self.no_local_box += other.no_local_box;
        self.with_boxes += other.with_boxes;
        self.box_is_transformed_local_box += other.box_is_transformed_local_box;
        self.radius_is_half_diagonal += other.radius_is_half_diagonal;
        self.centre_is_box_centre += other.centre_is_box_centre;
        self.has_second_block += other.has_second_block;
        self.second_block_agrees += other.second_block_agrees;
        self.roots += other.roots;
        self.root_local_equals_world += other.root_local_equals_world;
        self.layers += other.layers;
        self.table_children_complete += other.table_children_complete;
        self.table_roots_first += other.table_roots_first;
        for e in other.examples {
            if self.examples.len() < 12 {
                self.examples.push(e);
            }
        }
    }
}

pub fn transformed_box(local: ([f32; 3], [f32; 3]), m: &[f32; 16]) -> ([f32; 3], [f32; 3]) {
    let (lo, hi) = local;
    let mut out_min = [f32::INFINITY; 3];
    let mut out_max = [f32::NEG_INFINITY; 3];
    for cx in [lo[0], hi[0]] {
        for cy in [lo[1], hi[1]] {
            for cz in [lo[2], hi[2]] {
                let p = [
                    cx * m[0] + cy * m[4] + cz * m[8] + m[12],
                    cx * m[1] + cy * m[5] + cz * m[9] + m[13],
                    cx * m[2] + cy * m[6] + cz * m[10] + m[14],
                ];
                for k in 0..3 {
                    out_min[k] = out_min[k].min(p[k]);
                    out_max[k] = out_max[k].max(p[k]);
                }
            }
        }
    }
    (out_min, out_max)
}

fn read_u16(d: &[u8], at: usize) -> Option<u16> {
    d.get(at..at + 2).map(|b| u16::from_le_bytes(b.try_into().unwrap()))
}
fn read_u32(d: &[u8], at: usize) -> Option<u32> {
    d.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()))
}
fn read_f32(d: &[u8], at: usize) -> Option<f32> {
    d.get(at..at + 4).map(|b| f32::from_le_bytes(b.try_into().unwrap()))
}
fn read_vec3(d: &[u8], at: usize) -> Option<[f32; 3]> {
    Some([read_f32(d, at)?, read_f32(d, at + 4)?, read_f32(d, at + 8)?])
}
fn write_vec3(d: &mut [u8], at: usize, v: [f32; 3]) {
    for (k, c) in v.iter().enumerate() {
        d[at + k * 4..at + k * 4 + 4].copy_from_slice(&c.to_le_bytes());
    }
}

fn looks_like_matrix(d: &[u8], at: usize) -> bool {
    let Some(w) = read_f32(d, at + 60) else { return false };
    if (w - 1.0).abs() > 1e-3 {
        return false;
    }
    for row in 0..3 {
        let (a, b, c) = (
            read_f32(d, at + row * 16).unwrap_or(0.0),
            read_f32(d, at + row * 16 + 4).unwrap_or(0.0),
            read_f32(d, at + row * 16 + 8).unwrap_or(0.0),
        );
        let len = (a * a + b * b + c * c).sqrt();
        if !(0.001..=10_000.0).contains(&len) || !len.is_finite() {
            return false;
        }
    }
    true
}

fn read_matrix(d: &[u8], at: usize) -> Option<[f32; 16]> {
    let mut m = [0f32; 16];
    for (k, slot) in m.iter_mut().enumerate() {
        *slot = read_f32(d, at + k * 4)?;
    }
    Some(m)
}

fn write_matrix(d: &mut [u8], at: usize, m: &[f32; 16]) {
    for (k, v) in m.iter().enumerate() {
        d[at + k * 4..at + k * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
}

impl LrentDoc {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 14 || &bytes[0..8] != b"GENOMFLE" {
            bail!("not a GENOMFLE container");
        }
        let pool_off = read_u32(bytes, 10).ok_or_else(|| anyhow!("truncated header"))? as usize;
        let pool_start = 5usize
            .checked_add(pool_off)
            .filter(|p| *p <= bytes.len())
            .ok_or_else(|| anyhow!("string pool offset points outside the file"))?;

        let mut strings = Vec::new();
        {
            let count = read_u32(bytes, pool_start).unwrap_or(0) as usize;
            let mut off = pool_start + 4;
            for _ in 0..count {
                let Some(len) = read_u16(bytes, off) else { break };
                off += 2;
                let Some(raw) = bytes.get(off..off + len as usize) else { break };
                strings.push(String::from_utf8_lossy(raw).into_owned());
                off += len as usize;
            }
        }

        let content_start = 14usize;
        let content = &bytes[content_start..pool_start];
        let declared = read_u32(content, COUNT_OFFSET)
            .ok_or_else(|| anyhow!("file too small to hold an entity count"))? as usize;

        let end_marker = find_last(content, &MARK_END_CONTENT)
            .ok_or_else(|| anyhow!("no 0xDEADBEEF end marker — not a dynamic-layer .lrent"))?;
        if end_marker < 8 || content[end_marker - 8..end_marker] != [0xffu8; 8] {
            bail!("end marker is not preceded by the expected table terminator");
        }
        let table_end = end_marker - 8;

        let mut after_marker: Vec<usize> = Vec::new();
        {
            let mut i = ROOT_ENTITY_OFFSET;
            while let Some(found) = find_from(content, &MARK_END_OBJECT, i) {
                let next = found + 4;
                if next + 2 <= content.len() && content[next..next + 2] == RECORD_VERSION && next < table_end {
                    after_marker.push(next);
                }
                i = found + 4;
            }
        }

        let mut starts: Vec<usize> = Vec::new();
        if declared > 1 {
            let wanted = declared - 1;
            let first_marker = after_marker.first().copied().unwrap_or(table_end);
            let chain_len = |p: usize| 1 + after_marker.iter().filter(|m| **m > p).count();
            let plausible = |p: usize| {
                p + 24 + 64 <= content.len()
                    && content[p..p + 2] == RECORD_VERSION
                    && looks_like_matrix(content, p + 24)
                    && read_u16(content, p + 22).is_some_and(|i| (i as usize) < strings.len())
            };
            let mut first = None;
            for candidate in std::iter::once(FIRST_ENTITY_OFFSET)
                .chain(ROOT_ENTITY_OFFSET + 2..first_marker.min(content.len()))
            {
                if candidate < first_marker && plausible(candidate) && chain_len(candidate) == wanted {
                    first = Some(candidate);
                    break;
                }
            }
            let first = first.ok_or_else(|| {
                anyhow!("could not find where the first placed object begins (header declares {wanted})")
            })?;
            starts.push(first);
            starts.extend(after_marker.iter().filter(|m| **m > first));
        }

        let table_start = match starts.last() {
            Some(_) => find_last(&content[..table_end], &MARK_END_OBJECT)
                .map(|m| m + 4)
                .ok_or_else(|| anyhow!("entity records without an end marker"))?,
            None => table_end,
        };

        let mut container_sizes = Vec::new();
        for o in 0..ROOT_ENTITY_OFFSET.min(end_marker.saturating_sub(4)) {
            let Some(v) = read_u32(content, o) else { break };
            if o + 4 + v as usize == end_marker {
                container_sizes.push(o + content_start);
            }
        }
        if container_sizes.is_empty() {
            bail!(
                "no block in this container declares a length that reaches the end of the content — \
                 refusing to lengthen a file whose own structure this parser cannot account for"
            );
        }

        if (table_end - table_start) % 8 != 0 {
            bail!("parent/child table is not a whole number of 8-byte rows");
        }
        let mut pairs = Vec::new();
        let mut off = table_start;
        while off < table_end {
            pairs.push((read_u32(content, off).unwrap_or(0), read_u32(content, off + 4).unwrap_or(0)));
            off += 8;
        }

        if starts.len() + 1 != declared {
            bail!(
                "header says {declared} entities (root included) but {} records were found — refusing to edit a file this parser does not fully understand",
                starts.len()
            );
        }
        if pairs.len() != starts.len() {
            bail!("{} records but {} parent/child rows", starts.len(), pairs.len());
        }

        Ok(Self {
            data: bytes.to_vec(),
            content_start,
            pool_start,
            strings,
            starts: starts.iter().map(|s| s + content_start).collect(),
            table_start: table_start + content_start,
            pairs,
            container_sizes,
        })
    }

    pub fn strings(&self) -> &[String] {
        &self.strings
    }

    pub fn container_blocks(&self) -> usize {
        self.container_sizes.len()
    }

    fn record_end(&self, i: usize) -> usize {
        self.starts.get(i + 1).copied().unwrap_or(self.table_start)
    }

    fn parent_of(&self, index_1based: u32) -> u32 {
        self.pairs
            .iter()
            .find(|(_, c)| *c == index_1based)
            .map(|(p, _)| *p)
            .unwrap_or(0)
    }

    pub fn entities(&self) -> Vec<PlacedEntity> {
        let mut out = Vec::with_capacity(self.starts.len());
        for (i, &s) in self.starts.iter().enumerate() {
            let name = read_u16(&self.data, s + 22)
                .and_then(|idx| self.strings.get(idx as usize).cloned())
                .unwrap_or_default();
            let matrix = read_matrix(&self.data, s + 24).unwrap_or([0.0; 16]);
            let size = self
                .discover_fields(i)
                .and_then(|f| f.bbox)
                .and_then(|b| Some((read_vec3(&self.data, b)?, read_vec3(&self.data, b + 12)?)))
                .map(|(lo, hi)| [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]])
                .unwrap_or([0.0; 3]);
            out.push(PlacedEntity {
                index: i as u32 + 1,
                parent: self.parent_of(i as u32 + 1),
                name,
                position: [matrix[12], matrix[13], matrix[14]],
                matrix,
                size,
                start: s,
                end: self.record_end(i),
            });
        }
        out
    }

    pub fn audit_geometry(&self) -> GeometryAudit {
        let mut a = GeometryAudit::default();

        a.layers = 1;
        {
            let mut seen = vec![false; self.starts.len() + 1];
            let mut complete = self.pairs.len() == self.starts.len();
            for (_, child) in &self.pairs {
                match seen.get_mut(*child as usize) {
                    Some(slot) if *child >= 1 && !*slot => *slot = true,
                    _ => complete = false,
                }
            }
            if complete && seen.iter().skip(1).all(|s| *s) {
                a.table_children_complete = 1;
            }

            let roots = self.pairs.iter().take_while(|(p, _)| *p == 0).count();
            let no_stragglers = self.pairs.iter().skip(roots).all(|(p, _)| *p != 0);
            let ascending = self.pairs[..roots].windows(2).all(|w| w[0].1 < w[1].1);
            if no_stragglers && ascending {
                a.table_roots_first = 1;
            } else if a.examples.len() < 6 {
                a.examples.push(format!(
                    "table: {roots} leading layer-level rows, stragglers={}, ascending={ascending}",
                    !no_stragglers
                ));
            }
        }
        for i in 0..self.starts.len() {
            a.entities += 1;
            let Some(f) = self.discover_fields(i) else {
                a.no_fields += 1;
                continue;
            };
            let Some(matrix) = read_matrix(&self.data, f.world_matrix) else { continue };
            let (Some(bbox), Some(centre)) = (f.bbox, f.centre) else {
                a.no_box += 1;
                continue;
            };
            let Some(local_box) = self.local_box(i) else {
                a.no_local_box += 1;
                continue;
            };
            a.with_boxes += 1;

            let (stored_min, stored_max) = match (read_vec3(&self.data, bbox), read_vec3(&self.data, bbox + 12)) {
                (Some(lo), Some(hi)) => (lo, hi),
                _ => continue,
            };
            let (want_min, want_max) = transformed_box(local_box, &matrix);
            let extent = (0..3).fold(1.0f32, |m, k| m.max(stored_max[k] - stored_min[k]));
            let magnitude = (0..3).fold(1.0f32, |m, k| m.max(stored_max[k].abs()));
            let tol = 0.5 + extent * 2e-3 + magnitude * 1e-4;
            if (0..3).all(|k| (stored_min[k] - want_min[k]).abs() <= tol && (stored_max[k] - want_max[k]).abs() <= tol) {
                a.box_is_transformed_local_box += 1;
            } else if a.examples.len() < 6 {
                a.examples.push(format!(
                    "#{}: box stored [{:.1},{:.1},{:.1}]..[{:.1},{:.1},{:.1}] but local box under the matrix gives [{:.1},{:.1},{:.1}]..[{:.1},{:.1},{:.1}]",
                    i + 1,
                    stored_min[0], stored_min[1], stored_min[2], stored_max[0], stored_max[1], stored_max[2],
                    want_min[0], want_min[1], want_min[2], want_max[0], want_max[1], want_max[2],
                ));
            }

            let half = [
                (stored_max[0] - stored_min[0]) * 0.5,
                (stored_max[1] - stored_min[1]) * 0.5,
                (stored_max[2] - stored_min[2]) * 0.5,
            ];
            let want_radius = (half[0] * half[0] + half[1] * half[1] + half[2] * half[2]).sqrt();
            if let Some(stored_radius) = read_f32(&self.data, bbox + 24) {
                if (stored_radius - want_radius).abs() <= 0.05 + want_radius * 1e-3 {
                    a.radius_is_half_diagonal += 1;
                }
            }

            let want_centre = [
                (stored_min[0] + stored_max[0]) * 0.5,
                (stored_min[1] + stored_max[1]) * 0.5,
                (stored_min[2] + stored_max[2]) * 0.5,
            ];
            if let Some(stored_centre) = read_vec3(&self.data, centre) {
                if (0..3).all(|k| (stored_centre[k] - want_centre[k]).abs() <= tol) {
                    a.centre_is_box_centre += 1;
                }
            }

            if let (Some(b2), Some(c2)) = (f.bbox2, f.centre2) {
                a.has_second_block += 1;
                let same_box = self.data.get(b2..b2 + 28) == self.data.get(bbox..bbox + 28);
                let same_centre = self.data.get(c2..c2 + 12) == self.data.get(centre..centre + 12);
                if same_box && same_centre {
                    a.second_block_agrees += 1;
                }
            }

            if self.parent_of(i as u32 + 1) == 0 {
                a.roots += 1;
                if let Some(local) = f.local_matrix {
                    if self.data.get(local..local + 64) == self.data.get(f.world_matrix..f.world_matrix + 64) {
                        a.root_local_equals_world += 1;
                    }
                }
            }
        }
        a
    }

    fn write_geom_block(&mut self, bbox: usize, lo: [f32; 3], hi: [f32; 3]) {
        write_vec3(&mut self.data, bbox, lo);
        write_vec3(&mut self.data, bbox + 12, hi);
        let half = [(hi[0] - lo[0]) * 0.5, (hi[1] - lo[1]) * 0.5, (hi[2] - lo[2]) * 0.5];
        let radius = (half[0] * half[0] + half[1] * half[1] + half[2] * half[2]).sqrt();
        if let Some(slot) = self.data.get_mut(bbox + 24..bbox + 28) {
            slot.copy_from_slice(&radius.to_le_bytes());
        }
        write_vec3(
            &mut self.data,
            bbox + 28,
            [lo[0] + half[0], lo[1] + half[1], lo[2] + half[2]],
        );
    }

    fn shift_geom_block(&mut self, bbox: usize, delta: [f32; 3]) {
        for at in [bbox, bbox + 12, bbox + 28] {
            if let Some(v) = read_vec3(&self.data, at) {
                write_vec3(&mut self.data, at, [v[0] + delta[0], v[1] + delta[1], v[2] + delta[2]]);
            }
        }
    }

    fn local_box(&self, i: usize) -> Option<([f32; 3], [f32; 3])> {
        let s = *self.starts.get(i)?;
        let lo = read_vec3(&self.data, s + LOCAL_BOX_OFFSET)?;
        let hi = read_vec3(&self.data, s + LOCAL_BOX_OFFSET + 12)?;
        if (0..3).any(|k| !lo[k].is_finite() || !hi[k].is_finite() || lo[k] > hi[k]) {
            return None;
        }
        Some((lo, hi))
    }

    fn discover_fields(&self, i: usize) -> Option<Fields> {
        let s = *self.starts.get(i)?;
        let end = self.record_end(i);
        if !looks_like_matrix(&self.data, s + 24) {
            return None;
        }
        let pos = read_vec3(&self.data, s + 24 + 48)?;

        let (bbox, centre) = self.geom_block(s + 88, end, pos).unzip();

        let mut local = None;
        let mut p = s + 88;
        while p + 66 <= end {
            if self.data[p..p + 2] == RECORD_VERSION && looks_like_matrix(&self.data, p + 2) {
                local = Some(p + 2);
                break;
            }
            p += 1;
        }

        let (bbox2, centre2) =
            local.and_then(|l| self.geom_block(l + 64, end, pos)).unzip();

        Some(Fields { world_matrix: s + 24, local_matrix: local, bbox, centre, bbox2, centre2 })
    }

    fn geom_block(&self, o: usize, end: usize, pos: [f32; 3]) -> Option<(usize, usize)> {
        if o + 40 > end {
            return None;
        }
        let bmin = read_vec3(&self.data, o)?;
        let bmax = read_vec3(&self.data, o + 12)?;
        let c = read_vec3(&self.data, o + 28)?;
        let contains = |p: [f32; 3]| (0..3).all(|k| bmin[k] - 1.0 <= p[k] && p[k] <= bmax[k] + 1.0);
        ((0..3).all(|k| bmin[k] <= bmax[k]) && contains(pos) && contains(c)).then_some((o, o + 28))
    }

    pub fn set_transform(&mut self, index: u32, matrix: [f32; 16]) -> Result<()> {
        let i = self.record_index(index)?;
        let fields = self
            .discover_fields(i)
            .ok_or_else(|| anyhow!("entity {index} has no readable transform"))?;
        let old = read_matrix(&self.data, fields.world_matrix).unwrap_or([0.0; 16]);
        let delta = [matrix[12] - old[12], matrix[13] - old[13], matrix[14] - old[14]];

        write_matrix(&mut self.data, fields.world_matrix, &matrix);
        if self.parent_of(index) == 0 {
            if let Some(local) = fields.local_matrix {
                write_matrix(&mut self.data, local, &matrix);
            }
        }
        let mirrors = match (fields.bbox, fields.bbox2) {
            (Some(b1), Some(b2)) => self.data.get(b2..b2 + 40) == self.data.get(b1..b1 + 40),
            _ => false,
        };
        let recomputed = self.local_box(i).map(|local| transformed_box(local, &matrix));

        match (fields.bbox, recomputed) {
            (Some(b1), Some((lo, hi))) => {
                self.write_geom_block(b1, lo, hi);
                if let Some(b2) = fields.bbox2 {
                    if mirrors {
                        self.write_geom_block(b2, lo, hi);
                    } else {
                        self.shift_geom_block(b2, delta);
                    }
                }
            }
            _ => {
                for b in [fields.bbox, fields.bbox2].into_iter().flatten() {
                    self.shift_geom_block(b, delta);
                }
            }
        }
        match recomputed {
            Some((lo, hi)) => {
                self.grow_context_box(lo);
                self.grow_context_box(hi);
            }
            None => self.grow_context_box([matrix[12], matrix[13], matrix[14]]),
        }
        Ok(())
    }

    pub fn duplicate(&mut self, index: u32, matrix: [f32; 16]) -> Result<u32> {
        let i = self.record_index(index)?;
        if self.parent_of(index) != 0 {
            bail!("entity {index} is attached to another object — copy its parent instead");
        }
        let (start, end) = (self.starts[i], self.record_end(i));
        let mut record = self.data[start..end].to_vec();

        let mut seed = fresh_seed(self.data.len() as u64 ^ start as u64);
        write_guid(&mut record, 2, &mut seed);

        let insert_at = self.table_start;
        let new_index = self.starts.len() as u32 + 1;
        self.data.splice(insert_at..insert_at, record.iter().copied());
        let grew = record.len();
        self.starts.push(insert_at);
        self.table_start += grew;
        self.shift_pool_offset(grew as i64)?;
        self.set_entity_count(self.starts.len() as u32 + 1)?;

        let row_at = self.pairs.iter().take_while(|(p, _)| *p == 0).count();
        self.pairs.insert(row_at, (0, new_index));
        let byte_at = self.table_start + row_at * 8;
        let mut row = [0u8; 8];
        row[4..8].copy_from_slice(&new_index.to_le_bytes());
        self.data.splice(byte_at..byte_at, row.iter().copied());
        self.shift_pool_offset(8)?;

        self.set_transform(new_index, matrix)?;
        Ok(new_index)
    }

    pub fn keep_only(&mut self, keep: &[u32]) -> Result<()> {
        let total = self.starts.len() as u32;
        for k in keep {
            if *k == 0 || *k > total {
                bail!("no entity {k} in this layer");
            }
            if self.parent_of(*k) != 0 {
                bail!("entity {k} is attached to another object — it cannot be kept on its own");
            }
            if self.pairs.iter().any(|(p, _)| p == k) {
                bail!("entity {k} has objects attached to it — keeping it alone would break them");
            }
        }
        let mut keep_sorted: Vec<u32> = keep.to_vec();
        keep_sorted.sort_unstable();
        keep_sorted.dedup();
        if keep_sorted.is_empty() {
            bail!("keep_only needs at least one entity");
        }

        let mut kept_records: Vec<Vec<u8>> = Vec::new();
        for k in &keep_sorted {
            let i = (*k - 1) as usize;
            kept_records.push(self.data[self.starts[i]..self.record_end(i)].to_vec());
        }

        let first = self.starts[0];
        let old_records_len = self.table_start - first;
        let new_body: Vec<u8> = kept_records.concat();
        let new_len = new_body.len();
        self.data.splice(first..self.table_start, new_body);

        let table_start = first + new_len;
        let old_table_bytes = self.pairs.len() * 8;
        let mut table = Vec::with_capacity(keep_sorted.len() * 8);
        for (n, _) in keep_sorted.iter().enumerate() {
            table.extend_from_slice(&0u32.to_le_bytes());
            table.extend_from_slice(&(n as u32 + 1).to_le_bytes());
        }
        self.data.splice(table_start..table_start + old_table_bytes, table);

        let delta = new_len as i64 - old_records_len as i64
            + (keep_sorted.len() * 8) as i64
            - old_table_bytes as i64;
        self.shift_pool_offset(delta)?;

        self.starts.clear();
        let mut off = first;
        for r in &kept_records {
            self.starts.push(off);
            off += r.len();
        }
        self.table_start = table_start;
        self.pairs = (1..=keep_sorted.len() as u32).map(|n| (0, n)).collect();
        self.set_entity_count(keep_sorted.len() as u32 + 1)?;
        self.refit_context_box();
        Ok(())
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.data.clone()
    }

    fn record_index(&self, index_1based: u32) -> Result<usize> {
        if index_1based == 0 || index_1based as usize > self.starts.len() {
            bail!("no entity {index_1based} in this layer ({} placed)", self.starts.len());
        }
        Ok(index_1based as usize - 1)
    }

    fn set_entity_count(&mut self, count: u32) -> Result<()> {
        let at = self.content_start + COUNT_OFFSET;
        self.data
            .get_mut(at..at + 4)
            .ok_or_else(|| anyhow!("file too small for the entity count"))?
            .copy_from_slice(&count.to_le_bytes());
        Ok(())
    }

    fn shift_pool_offset(&mut self, delta: i64) -> Result<()> {
        let current = read_u32(&self.data, 10).ok_or_else(|| anyhow!("truncated header"))? as i64;
        let next = current + delta;
        if next < 0 || next > u32::MAX as i64 {
            bail!("string pool offset would go out of range");
        }
        self.data[10..14].copy_from_slice(&(next as u32).to_le_bytes());
        self.pool_start = (self.pool_start as i64 + delta) as usize;

        for at in self.container_sizes.clone() {
            let size = read_u32(&self.data, at).ok_or_else(|| anyhow!("truncated container block size"))? as i64;
            let grown = size + delta;
            if grown < 0 || grown > u32::MAX as i64 {
                bail!("a container block length would go out of range");
            }
            self.data[at..at + 4].copy_from_slice(&(grown as u32).to_le_bytes());
        }
        Ok(())
    }

    fn grow_context_box(&mut self, point: [f32; 3]) {
        for base in CONTEXT_BOX_OFFSETS {
            let at = self.content_start + base;
            let (Some(mut lo), Some(mut hi)) = (read_vec3(&self.data, at), read_vec3(&self.data, at + 12))
            else {
                continue;
            };
            if (0..3).any(|k| !lo[k].is_finite() || !hi[k].is_finite() || lo[k] > hi[k]) {
                continue;
            }
            let mut changed = false;
            for k in 0..3 {
                if point[k] < lo[k] {
                    lo[k] = point[k];
                    changed = true;
                }
                if point[k] > hi[k] {
                    hi[k] = point[k];
                    changed = true;
                }
            }
            if changed {
                write_vec3(&mut self.data, at, lo);
                write_vec3(&mut self.data, at + 12, hi);
            }
        }
    }

    fn refit_context_box(&mut self) {
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        let mut any = false;
        for e in self.entities() {
            for k in 0..3 {
                lo[k] = lo[k].min(e.position[k]);
                hi[k] = hi[k].max(e.position[k]);
            }
            any = true;
        }
        if !any {
            return;
        }
        const PAD: f32 = 3000.0;
        for base in CONTEXT_BOX_OFFSETS {
            let at = self.content_start + base;
            write_vec3(&mut self.data, at, [lo[0] - PAD, lo[1] - PAD, lo[2] - PAD]);
            write_vec3(&mut self.data, at + 12, [hi[0] + PAD, hi[1] + PAD, hi[2] + PAD]);
        }
    }
}

fn find_from(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from >= hay.len() {
        return None;
    }
    hay[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

fn find_last(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if hay.len() < needle.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).rev().find(|&i| &hay[i..i + needle.len()] == needle)
}

fn fresh_seed(mix: u64) -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E3779B97F4A7C15);
    now ^ mix.rotate_left(17) ^ 0xD1B54A32D192ED03
}

fn next_u64(seed: &mut u64) -> u64 {
    let mut x = *seed;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *seed = x;
    x
}

fn write_guid(record: &mut [u8], at: usize, seed: &mut u64) {
    if at + 16 > record.len() {
        return;
    }
    let a = next_u64(seed).to_le_bytes();
    let b = next_u64(seed).to_le_bytes();
    record[at..at + 8].copy_from_slice(&a);
    record[at + 8..at + 16].copy_from_slice(&b);
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq)]
    pub(crate) enum Layout {
        Dynamic,
        StaticLevelmesh,
    }

    pub(crate) fn build_layer(entities: &[(&str, [f32; 3])]) -> Vec<u8> {
        build_layer_with(entities, Layout::Dynamic)
    }

    pub(crate) fn build_static_layer(entities: &[(&str, [f32; 3])]) -> Vec<u8> {
        build_layer_with(entities, Layout::StaticLevelmesh)
    }

    pub(crate) fn build_layer_with(entities: &[(&str, [f32; 3])], layout: Layout) -> Vec<u8> {
        let mut strings: Vec<String> = vec!["gCDynamicLayer".into(), "ContextBox".into(), "bCBox".into()];
        let mut content = vec![0u8; FIRST_ENTITY_OFFSET];
        content[0..4].copy_from_slice(&[0xde, 0xfa, 0xde, 0xd0]);
        for base in CONTEXT_BOX_OFFSETS {
            write_vec3(&mut content, base, [-10.0, -10.0, -10.0]);
            write_vec3(&mut content, base + 12, [10.0, 10.0, 10.0]);
        }
        content[COUNT_OFFSET..COUNT_OFFSET + 4].copy_from_slice(&(entities.len() as u32 + 1).to_le_bytes());
        content[198..200].copy_from_slice(&RECORD_VERSION);

        for (name, pos) in entities {
            let name_index = strings.len() as u16;
            strings.push((*name).into());
            let mut rec = vec![0u8; 24];
            rec[0..2].copy_from_slice(&RECORD_VERSION);
            rec[2..18].copy_from_slice(&[7u8; 16]);
            rec[22..24].copy_from_slice(&name_index.to_le_bytes());
            let mut m = [0f32; 16];
            m[0] = 1.0;
            m[5] = 1.0;
            m[10] = 1.0;
            m[15] = 1.0;
            m[12] = pos[0];
            m[13] = pos[1];
            m[14] = pos[2];
            let mut mb = vec![0u8; 64];
            write_matrix(&mut mb, 0, &m);
            rec.extend_from_slice(&mb);
            let mut geom = vec![0u8; 40];
            write_vec3(&mut geom, 0, [pos[0] - 1.0, pos[1] - 1.0, pos[2] - 1.0]);
            write_vec3(&mut geom, 12, [pos[0] + 1.0, pos[1] + 1.0, pos[2] + 1.0]);
            geom[24..28].copy_from_slice(&(3f32).sqrt().to_le_bytes());
            write_vec3(&mut geom, 28, *pos);
            rec.extend_from_slice(&geom);
            let mut local_bbox = vec![0u8; 24];
            write_vec3(&mut local_bbox, 0, [-1.0, -1.0, -1.0]);
            write_vec3(&mut local_bbox, 12, [1.0, 1.0, 1.0]);
            rec.extend_from_slice(&local_bbox);
            match layout {
                Layout::Dynamic => {
                    rec.extend_from_slice(&[0u8; 3]);
                    rec.extend_from_slice(&[9u8; 16]);
                    rec.extend_from_slice(&[0u8; 4]);
                    rec.extend_from_slice(&RECORD_VERSION);
                    rec.extend_from_slice(&mb);
                }
                Layout::StaticLevelmesh => {
                    rec.extend_from_slice(&[1u8, 0, 0]);
                    rec.extend_from_slice(&RECORD_VERSION);
                    rec.extend_from_slice(&mb);
                }
            }
            rec.extend_from_slice(&geom);
            rec.extend_from_slice(&MARK_END_OBJECT);
            content.extend_from_slice(&rec);
        }
        for n in 1..=entities.len() as u32 {
            content.extend_from_slice(&0u32.to_le_bytes());
            content.extend_from_slice(&n.to_le_bytes());
        }
        content.extend_from_slice(&[0xffu8; 8]);
        content.extend_from_slice(&MARK_END_CONTENT);
        content.push(0x01);

        let end_marker = content.len() - 5;
        for at in [19usize, 93] {
            let size = (end_marker - (at + 4)) as u32;
            content[at..at + 4].copy_from_slice(&size.to_le_bytes());
            content[at - 2..at].copy_from_slice(&201u16.to_le_bytes());
        }

        let mut pool = Vec::new();
        pool.extend_from_slice(&(strings.len() as u32).to_le_bytes());
        for s in &strings {
            pool.extend_from_slice(&(s.len() as u16).to_le_bytes());
            pool.extend_from_slice(s.as_bytes());
        }

        let mut out = Vec::new();
        out.extend_from_slice(b"GENOMFLE");
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&((content.len() + 14 - 5) as u32).to_le_bytes());
        out.extend_from_slice(&content);
        out.extend_from_slice(&pool);
        out
    }

    #[test]
    fn reads_every_placed_object_with_its_name_and_position() {
        let bytes = build_layer(&[("Ship_Large", [100.0, 5.0, 200.0]), ("Barrel", [1.0, 2.0, 3.0])]);
        let doc = LrentDoc::parse(&bytes).unwrap();
        let e = doc.entities();
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].name, "Ship_Large");
        assert_eq!(e[0].index, 1);
        assert_eq!(e[0].position, [100.0, 5.0, 200.0]);
        assert_eq!(e[1].name, "Barrel");
        assert_eq!(e[1].parent, 0);
        assert_eq!(e[0].size, [2.0, 2.0, 2.0]);
    }

    #[test]
    fn a_header_that_disagrees_with_the_records_is_refused_not_guessed_at() {
        let mut bytes = build_layer(&[("Ship_Large", [1.0, 1.0, 1.0])]);
        let at = 14 + COUNT_OFFSET;
        bytes[at..at + 4].copy_from_slice(&9u32.to_le_bytes());
        let err = LrentDoc::parse(&bytes).unwrap_err().to_string();
        assert!(err.contains("declares 8") || err.contains("refusing"), "{err}");
    }

    #[test]
    fn moving_an_object_moves_its_world_matrix_local_matrix_box_and_centre_together() {
        let bytes = build_layer(&[("Ship_Large", [100.0, 5.0, 200.0])]);
        let mut doc = LrentDoc::parse(&bytes).unwrap();
        let before = doc.entities()[0].clone();
        let mut m = before.matrix;
        m[12] = 150.0;
        m[13] = 25.0;
        m[14] = 260.0;
        doc.set_transform(1, m).unwrap();

        let out = doc.to_bytes();
        assert_eq!(out.len(), bytes.len(), "an in-place move must not change the file size");
        let reparsed = LrentDoc::parse(&out).unwrap();
        assert_eq!(reparsed.entities()[0].position, [150.0, 25.0, 260.0]);

        let s = before.start;
        assert_eq!(read_vec3(&out, s + 177 + 48).unwrap(), [150.0, 25.0, 260.0]);
        assert_eq!(read_vec3(&out, s + 88).unwrap(), [149.0, 24.0, 259.0]);
        assert_eq!(read_vec3(&out, s + 100).unwrap(), [151.0, 26.0, 261.0]);
        assert_eq!(read_vec3(&out, s + 116).unwrap(), [150.0, 25.0, 260.0]);
    }

    #[test]
    fn a_copy_differs_from_its_original_only_where_the_engine_itself_differs() {
        for layout in [Layout::Dynamic, Layout::StaticLevelmesh] {
            let bytes = build_layer_with(&[("Ship_Large", [100.0, 5.0, 200.0])], layout);
            let mut doc = LrentDoc::parse(&bytes).unwrap();
            let mut m = doc.entities()[0].matrix;
            m[12] = 150.0;
            let copy_index = doc.duplicate(1, m).unwrap();

            let out = doc.to_bytes();
            let reparsed = LrentDoc::parse(&out).unwrap();
            let all = reparsed.entities();
            let (orig, copy) = (all[0].clone(), all[copy_index as usize - 1].clone());
            let len = orig.end - orig.start;
            assert_eq!(copy.end - copy.start, len, "{layout:?}: the copy is a different size");

            let local = match layout {
                Layout::Dynamic => 177,
                Layout::StaticLevelmesh => 157,
            };
            let may_differ = [
                (2, 18),
                (24, 88),
                (88, 112),
                (116, 128),
                (local, local + 64),
                (local + 64, local + 88),
                (local + 92, local + 104),
            ];
            for k in 0..len {
                let (a, b) = (out[orig.start + k], out[copy.start + k]);
                if a == b {
                    continue;
                }
                assert!(
                    may_differ.iter().any(|(lo, hi)| (*lo..*hi).contains(&k)),
                    "{layout:?}: the copy differs from its original at S+{k} \
                     ({a:#04x} -> {b:#04x}) — not a field a copy is allowed to change",
                );
            }
            assert_eq!(
                &out[orig.start + 128..orig.start + 152],
                &out[copy.start + 128..copy.start + 152],
                "{layout:?}: an object's own extent is the same in every instance of it",
            );
        }
    }

    #[test]
    fn the_container_blocks_still_end_at_the_trailer_after_a_copy_is_added() {
        for layout in [Layout::Dynamic, Layout::StaticLevelmesh] {
            let bytes = build_layer_with(&[("Ship_Large", [100.0, 5.0, 200.0]), ("Barrel", [1.0, 2.0, 3.0])], layout);
            let mut doc = LrentDoc::parse(&bytes).unwrap();
            let m = doc.entities()[0].matrix;
            doc.duplicate(1, m).unwrap();
            let out = doc.to_bytes();

            assert!(out.len() > bytes.len(), "{layout:?}: a copy must make the file longer");

            let pool_start = 5 + read_u32(&out, 10).unwrap() as usize;
            let content = &out[14..pool_start];
            let end_marker = find_last(content, &MARK_END_CONTENT).unwrap();
            for at in [19usize, 93] {
                let size = read_u32(content, at).unwrap() as usize;
                assert_eq!(
                    at + 4 + size,
                    end_marker,
                    "{layout:?}: the container block at content+{at} still declares its old length",
                );
            }

            let reparsed = LrentDoc::parse(&out).unwrap();
            assert_eq!(reparsed.entities().len(), 3);
        }
    }

    #[test]
    fn the_container_blocks_follow_a_deletion_too() {
        let bytes = build_static_layer(&[
            ("Ship_Large", [100.0, 5.0, 200.0]),
            ("Barrel", [1.0, 2.0, 3.0]),
            ("Crate", [4.0, 5.0, 6.0]),
        ]);
        let mut doc = LrentDoc::parse(&bytes).unwrap();
        doc.keep_only(&[2]).unwrap();
        let out = doc.to_bytes();

        assert!(out.len() < bytes.len(), "keeping one of three must make the file shorter");
        let pool_start = 5 + read_u32(&out, 10).unwrap() as usize;
        let content = &out[14..pool_start];
        let end_marker = find_last(content, &MARK_END_CONTENT).unwrap();
        for at in [19usize, 93] {
            let size = read_u32(content, at).unwrap() as usize;
            assert_eq!(at + 4 + size, end_marker, "the container block at content+{at} did not shrink with the file");
        }
        assert_eq!(LrentDoc::parse(&out).unwrap().entities().len(), 1);
    }

    #[test]
    fn a_file_whose_container_declares_nothing_is_refused() {
        let mut bytes = build_layer(&[("Ship_Large", [1.0, 1.0, 1.0])]);
        for at in [19usize, 93] {
            let of = 14 + at;
            bytes[of..of + 4].copy_from_slice(&0u32.to_le_bytes());
        }
        let err = LrentDoc::parse(&bytes).unwrap_err().to_string();
        assert!(err.contains("reaches the end of the content"), "{err}");
    }

    #[test]
    fn the_fixture_obeys_every_rule_the_shipped_game_obeys() {
        for layout in [Layout::Dynamic, Layout::StaticLevelmesh] {
            let bytes = build_layer_with(&[("Ship_Large", [100.0, 5.0, 200.0]), ("Barrel", [1.0, 2.0, 3.0])], layout);
            let a = LrentDoc::parse(&bytes).unwrap().audit_geometry();
            assert_eq!(a.entities, 2, "{layout:?}");
            assert_eq!(a.with_boxes, 2, "{layout:?}: both records must have a usable box");
            assert_eq!(a.box_is_transformed_local_box, 2, "{layout:?}: world box must be the local box under the matrix");
            assert_eq!(a.radius_is_half_diagonal, 2, "{layout:?}: radius must be the box's half-diagonal");
            assert_eq!(a.centre_is_box_centre, 2, "{layout:?}: centre must be the box's centre");
            assert_eq!(a.has_second_block, 2, "{layout:?}");
            assert_eq!(a.second_block_agrees, 2, "{layout:?}");
            assert_eq!(a.root_local_equals_world, 2, "{layout:?}");
            assert_eq!(a.table_children_complete, 1, "{layout:?}: the table must name every entity once");
            assert_eq!(a.table_roots_first, 1, "{layout:?}: layer-level rows come first, ascending");
        }
    }

    #[test]
    fn a_copy_obeys_the_same_rules_as_the_original() {
        for layout in [Layout::Dynamic, Layout::StaticLevelmesh] {
            let bytes = build_layer_with(&[("Ship_Large", [100.0, 5.0, 200.0])], layout);
            let mut doc = LrentDoc::parse(&bytes).unwrap();
            doc.duplicate(1, yaw(30.0, [900.0, 5.0, 900.0], 2.5)).unwrap();
            let a = LrentDoc::parse(&doc.to_bytes()).unwrap().audit_geometry();
            assert_eq!(a.with_boxes, 2, "{layout:?}");
            assert_eq!(a.box_is_transformed_local_box, 2, "{layout:?}: the copy's box must fit its new turn and size");
            assert_eq!(a.radius_is_half_diagonal, 2, "{layout:?}");
            assert_eq!(a.centre_is_box_centre, 2, "{layout:?}");
            assert_eq!(a.second_block_agrees, 2, "{layout:?}");
            assert_eq!(a.table_children_complete, 1, "{layout:?}");
            assert_eq!(a.table_roots_first, 1, "{layout:?}");
        }
    }

    fn yaw(degrees: f32, pos: [f32; 3], scale: f32) -> [f32; 16] {
        let r = degrees.to_radians();
        let (c, s) = (r.cos() * scale, r.sin() * scale);
        [c, 0.0, -s, 0.0, 0.0, scale, 0.0, 0.0, s, 0.0, c, 0.0, pos[0], pos[1], pos[2], 1.0]
    }

    #[test]
    fn turning_an_object_rebuilds_the_shape_of_its_box() {
        let bytes = build_static_layer(&[("Ship_Large", [100.0, 5.0, 200.0])]);
        let mut doc = LrentDoc::parse(&bytes).unwrap();
        let s = doc.entities()[0].start;

        doc.set_transform(1, yaw(45.0, [100.0, 5.0, 200.0], 1.0)).unwrap();
        let out = doc.to_bytes();

        let root2 = 2f32.sqrt();
        let lo = read_vec3(&out, s + 88).unwrap();
        let hi = read_vec3(&out, s + 100).unwrap();
        for (got, want) in lo.iter().zip([100.0 - root2, 4.0, 200.0 - root2]) {
            assert!((got - want).abs() < 1e-3, "box min {lo:?} should reach √2 on X and Z");
        }
        for (got, want) in hi.iter().zip([100.0 + root2, 6.0, 200.0 + root2]) {
            assert!((got - want).abs() < 1e-3, "box max {hi:?} should reach √2 on X and Z");
        }

        let radius = f32::from_le_bytes(out[s + 112..s + 116].try_into().unwrap());
        assert!((radius - 5f32.sqrt()).abs() < 1e-3, "radius {radius} should be the new half-diagonal √5");
        assert_eq!(read_vec3(&out, s + 116).unwrap(), [100.0, 5.0, 200.0], "the centre is the box's centre");

        assert_eq!(&out[s + 157 + 64..s + 157 + 104], &out[s + 88..s + 128], "the second block must agree");
    }

    #[test]
    fn resizing_an_object_grows_its_box_and_its_radius() {
        let bytes = build_static_layer(&[("Ship_Large", [0.0, 0.0, 0.0])]);
        let mut doc = LrentDoc::parse(&bytes).unwrap();
        let s = doc.entities()[0].start;
        doc.set_transform(1, yaw(0.0, [0.0, 0.0, 0.0], 3.0)).unwrap();
        let out = doc.to_bytes();

        assert_eq!(read_vec3(&out, s + 88).unwrap(), [-3.0, -3.0, -3.0]);
        assert_eq!(read_vec3(&out, s + 100).unwrap(), [3.0, 3.0, 3.0]);
        let radius = f32::from_le_bytes(out[s + 112..s + 116].try_into().unwrap());
        assert!((radius - 27f32.sqrt()).abs() < 1e-3, "radius {radius} should be √27 for a ±3 box");
    }

    #[test]
    fn a_second_block_that_is_not_a_copy_is_only_moved() {
        let bytes = build_static_layer(&[("Ship_Large", [0.0, 0.0, 0.0])]);
        let mut doc = LrentDoc::parse(&bytes).unwrap();
        let s = doc.entities()[0].start;
        let second = s + 157 + 64;
        write_vec3(&mut doc.data, second, [-9.0, -9.0, -9.0]);
        write_vec3(&mut doc.data, second + 12, [9.0, 9.0, 9.0]);

        doc.set_transform(1, yaw(90.0, [10.0, 0.0, 0.0], 1.0)).unwrap();
        let out = doc.to_bytes();

        assert_eq!(read_vec3(&out, second).unwrap(), [1.0, -9.0, -9.0], "second block min should be shifted, not rebuilt");
        assert_eq!(read_vec3(&out, second + 12).unwrap(), [19.0, 9.0, 9.0], "second block max should be shifted, not rebuilt");
        assert_eq!(read_vec3(&out, s + 88).unwrap(), [9.0, -1.0, -1.0], "the first block should be rebuilt around the turn");
    }

    #[test]
    fn the_layer_box_grows_around_the_whole_object_not_its_origin() {
        let bytes = build_static_layer(&[("Ship_Large", [0.0, 0.0, 0.0])]);
        let mut doc = LrentDoc::parse(&bytes).unwrap();
        doc.set_transform(1, yaw(0.0, [100.0, 0.0, 0.0], 1.0)).unwrap();
        let out = doc.to_bytes();

        for base in CONTEXT_BOX_OFFSETS {
            let hi = read_vec3(&out, 14 + base + 12).unwrap();
            assert!(hi[0] >= 101.0, "context box max {hi:?} must reach the object's far corner, not just its origin");
        }
    }

    #[test]
    fn moving_an_object_moves_both_copies_of_its_box() {
        let bytes = build_static_layer(&[("Ship_Large", [100.0, 5.0, 200.0])]);
        let mut doc = LrentDoc::parse(&bytes).unwrap();
        let s = doc.entities()[0].start;
        let mut m = doc.entities()[0].matrix;
        m[12] = 150.0;
        m[13] = 25.0;
        m[14] = 260.0;
        doc.set_transform(1, m).unwrap();
        let out = doc.to_bytes();

        assert_eq!(read_vec3(&out, s + 88).unwrap(), [149.0, 24.0, 259.0]);
        assert_eq!(read_vec3(&out, s + 116).unwrap(), [150.0, 25.0, 260.0]);
        assert_eq!(read_vec3(&out, s + 157 + 64).unwrap(), [149.0, 24.0, 259.0]);
        assert_eq!(read_vec3(&out, s + 157 + 76).unwrap(), [151.0, 26.0, 261.0]);
        assert_eq!(read_vec3(&out, s + 157 + 92).unwrap(), [150.0, 25.0, 260.0]);
        assert_eq!(
            &out[s + 128..s + 152],
            &bytes[s + 128..s + 152],
            "the local bounding box does not travel with the object",
        );
    }

    #[test]
    fn moving_an_object_outside_the_layer_box_widens_the_box() {
        let bytes = build_layer(&[("Ship_Large", [1.0, 1.0, 1.0])]);
        let mut doc = LrentDoc::parse(&bytes).unwrap();
        let mut m = doc.entities()[0].matrix;
        m[12] = 5000.0;
        doc.set_transform(1, m).unwrap();
        let out = doc.to_bytes();
        for base in CONTEXT_BOX_OFFSETS {
            assert!(read_vec3(&out, 14 + base + 12).unwrap()[0] >= 5000.0);
        }
    }

    #[test]
    fn duplicating_an_object_adds_a_record_a_table_row_and_a_count_the_parser_agrees_with() {
        let bytes = build_layer(&[("Ship_Large", [100.0, 5.0, 200.0]), ("Barrel", [1.0, 2.0, 3.0])]);
        let mut doc = LrentDoc::parse(&bytes).unwrap();
        let mut m = doc.entities()[0].matrix;
        m[12] = 900.0;
        m[14] = 900.0;
        let new_index = doc.duplicate(1, m).unwrap();
        assert_eq!(new_index, 3);

        let out = doc.to_bytes();
        let reparsed = LrentDoc::parse(&out).unwrap();
        let e = reparsed.entities();
        assert_eq!(e.len(), 3);
        assert_eq!(e[2].name, "Ship_Large");
        assert_eq!(e[2].position, [900.0, 5.0, 900.0]);
        assert_eq!(e[0].position, [100.0, 5.0, 200.0], "the original must stay where it was");
        assert_eq!(read_u32(&out, 14 + COUNT_OFFSET).unwrap(), 4);
    }

    #[test]
    fn a_duplicate_gets_its_own_ids() {
        let bytes = build_layer(&[("Ship_Large", [1.0, 1.0, 1.0])]);
        let mut doc = LrentDoc::parse(&bytes).unwrap();
        let m = doc.entities()[0].matrix;
        doc.duplicate(1, m).unwrap();
        let out = doc.to_bytes();
        let reparsed = LrentDoc::parse(&out).unwrap();
        let (a, b) = (reparsed.entities()[0].start, reparsed.entities()[1].start);
        assert_ne!(&out[a + 2..a + 18], &out[b + 2..b + 18], "the copy must not reuse the original's guid");
        assert_eq!(
            &out[a + 155..a + 171],
            &out[b + 155..b + 171],
            "a copy points at the same template as its original",
        );
    }

    #[test]
    fn keeping_one_object_leaves_a_valid_single_object_layer() {
        let bytes = build_layer(&[
            ("Ship_Large", [100.0, 5.0, 200.0]),
            ("Barrel", [1.0, 2.0, 3.0]),
            ("Crate", [4.0, 5.0, 6.0]),
        ]);
        let mut doc = LrentDoc::parse(&bytes).unwrap();
        doc.keep_only(&[2]).unwrap();
        let out = doc.to_bytes();
        let reparsed = LrentDoc::parse(&out).unwrap();
        let e = reparsed.entities();
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].name, "Barrel");
        assert_eq!(e[0].index, 1);
        assert_eq!(read_u32(&out, 14 + COUNT_OFFSET).unwrap(), 2);
        assert!(out.len() < bytes.len());
    }

    #[test]
    fn stripping_a_layer_refits_its_streaming_box_around_what_is_left() {
        let bytes = build_layer(&[("Ship_Large", [10000.0, 0.0, 0.0]), ("Barrel", [1.0, 2.0, 3.0])]);
        let mut doc = LrentDoc::parse(&bytes).unwrap();
        doc.keep_only(&[2]).unwrap();
        let out = doc.to_bytes();
        let hi = read_vec3(&out, 14 + CONTEXT_BOX_OFFSETS[0] + 12).unwrap();
        assert!(hi[0] < 10000.0, "the box should no longer stretch to the object we removed");
    }

    #[test]
    fn asking_for_an_object_that_is_not_there_is_an_error_not_a_panic() {
        let bytes = build_layer(&[("Ship_Large", [1.0, 1.0, 1.0])]);
        let mut doc = LrentDoc::parse(&bytes).unwrap();
        assert!(doc.set_transform(7, [0.0; 16]).is_err());
        assert!(doc.duplicate(0, [0.0; 16]).is_err());
    }

    #[test]
    fn non_lrent_input_is_rejected() {
        assert!(LrentDoc::parse(b"not a file at all").is_err());
    }
}
