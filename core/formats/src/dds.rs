//! DDS textures: DXT1/DXT3/DXT5 and uncompressed pixel data, decoded to RGBA.

use anyhow::{anyhow, bail, Result};
use ddsfile::{D3DFormat, DataFormat, Dds, NewD3dParams, PixelFormat, PixelFormatFlags};
use texpresso::{Format as BcFormat, Params as BcParams};

pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

fn bc_format_for(fmt: D3DFormat) -> Option<BcFormat> {
    match fmt {
        D3DFormat::DXT1 => Some(BcFormat::Bc1),
        D3DFormat::DXT2 | D3DFormat::DXT3 => Some(BcFormat::Bc2),
        D3DFormat::DXT4 | D3DFormat::DXT5 => Some(BcFormat::Bc3),
        _ => None,
    }
}

pub fn resolve_format(dds: &Dds) -> Option<D3DFormat> {
    if let Some(fmt) = dds.get_d3d_format() {
        return Some(fmt);
    }
    let spf = &dds.header.spf;
    let is_luminance_only = spf.flags.contains(PixelFormatFlags::LUMINANCE)
        && !spf.flags.contains(PixelFormatFlags::ALPHA)
        && !spf.flags.contains(PixelFormatFlags::ALPHA_PIXELS);
    if is_luminance_only && spf.rgb_bit_count == Some(8) {
        return Some(D3DFormat::L8);
    }
    None
}

fn single_channel_mask(fmt: D3DFormat) -> Option<u32> {
    match fmt {
        D3DFormat::L8 => fmt.r_bit_mask(),
        D3DFormat::A8 => fmt.a_bit_mask(),
        _ => None,
    }
}

fn bytes_per_pixel(fmt: D3DFormat) -> Result<usize> {
    let bpp = fmt
        .get_bits_per_pixel()
        .ok_or_else(|| anyhow!("format {:?} has no fixed bits-per-pixel (compressed?)", fmt))?;
    if bpp % 8 != 0 {
        bail!("format {:?} has a non-byte-aligned bit depth ({} bits) — not supported", fmt, bpp);
    }
    Ok((bpp / 8) as usize)
}

fn extract_channel(pixel: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let bits = mask.count_ones();
    let value = (pixel & mask) >> shift;
    if bits >= 8 {
        (value >> (bits - 8)) as u8
    } else {
        ((value << (8 - bits)) | (value >> (bits.saturating_sub(8).min(bits)))) as u8
    }
}

fn place_channel(value: u8, mask: u32) -> u32 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let bits = mask.count_ones();
    let scaled = if bits >= 8 {
        (value as u32) << (bits - 8)
    } else {
        (value as u32) >> (8 - bits)
    };
    (scaled << shift) & mask
}

fn unpack_uncompressed(data: &[u8], width: u32, height: u32, fmt: D3DFormat) -> Result<Vec<u8>> {
    let bpp = bytes_per_pixel(fmt)?;
    let pixel_count = (width as usize) * (height as usize);
    let expected = pixel_count * bpp;
    if data.len() < expected {
        bail!(
            "uncompressed DDS data too short: got {} bytes, need {}",
            data.len(),
            expected
        );
    }

    let mut out = Vec::with_capacity(pixel_count * 4);
    if let Some(value_mask) = single_channel_mask(fmt) {
        for chunk in data[..expected].chunks_exact(bpp) {
            let mut buf = [0u8; 4];
            buf[..bpp].copy_from_slice(chunk);
            let pixel = u32::from_le_bytes(buf);
            let v = extract_channel(pixel, value_mask);
            out.extend_from_slice(&[v, v, v, 255]);
        }
    } else {
        let r_mask = fmt.r_bit_mask().unwrap_or(0);
        let g_mask = fmt.g_bit_mask().unwrap_or(0);
        let b_mask = fmt.b_bit_mask().unwrap_or(0);
        let a_mask = fmt.a_bit_mask();
        for chunk in data[..expected].chunks_exact(bpp) {
            let mut buf = [0u8; 4];
            buf[..bpp].copy_from_slice(chunk);
            let pixel = u32::from_le_bytes(buf);
            out.push(extract_channel(pixel, r_mask));
            out.push(extract_channel(pixel, g_mask));
            out.push(extract_channel(pixel, b_mask));
            out.push(a_mask.map(|m| extract_channel(pixel, m)).unwrap_or(255));
        }
    }
    Ok(out)
}

fn pack_uncompressed(rgba: &[u8], fmt: D3DFormat) -> Result<Vec<u8>> {
    let bpp = bytes_per_pixel(fmt)?;
    let mut out = Vec::with_capacity(rgba.len() / 4 * bpp);

    if let Some(value_mask) = single_channel_mask(fmt) {
        for px in rgba.chunks_exact(4) {
            let pixel = place_channel(px[0], value_mask);
            out.extend_from_slice(&pixel.to_le_bytes()[..bpp]);
        }
    } else {
        let r_mask = fmt.r_bit_mask().unwrap_or(0);
        let g_mask = fmt.g_bit_mask().unwrap_or(0);
        let b_mask = fmt.b_bit_mask().unwrap_or(0);
        let a_mask = fmt.a_bit_mask();
        for px in rgba.chunks_exact(4) {
            let mut pixel = place_channel(px[0], r_mask) | place_channel(px[1], g_mask) | place_channel(px[2], b_mask);
            if let Some(m) = a_mask {
                pixel |= place_channel(px[3], m);
            }
            out.extend_from_slice(&pixel.to_le_bytes()[..bpp]);
        }
    }
    Ok(out)
}

pub fn decode(dds_bytes: &[u8]) -> Result<DecodedImage> {
    let dds = Dds::read(dds_bytes)?;
    let width = dds.get_width();
    let height = dds.get_height();
    let format = resolve_format(&dds).ok_or_else(|| anyhow!("unrecognized or non-D3D DDS pixel format"))?;

    let main_size = if let Some(bc) = bc_format_for(format) {
        bc.compressed_size(width as usize, height as usize)
    } else {
        (width as usize) * (height as usize) * bytes_per_pixel(format)?
    };
    if dds.data.len() < main_size {
        bail!(
            "DDS data too short for top-level mip: got {} bytes, need {main_size}",
            dds.data.len()
        );
    }
    let level0 = &dds.data[..main_size];

    let rgba = if let Some(bc) = bc_format_for(format) {
        let mut out = vec![0u8; (width as usize) * (height as usize) * 4];
        bc.decompress(level0, width as usize, height as usize, &mut out);
        out
    } else {
        unpack_uncompressed(level0, width, height, format)?
    };

    Ok(DecodedImage {
        width,
        height,
        rgba,
    })
}

fn downsample_half(rgba: &[u8], width: u32, height: u32) -> (u32, u32, Vec<u8>) {
    let nw = (width / 2).max(1);
    let nh = (height / 2).max(1);
    let mut out = vec![0u8; (nw * nh * 4) as usize];
    for y in 0..nh {
        for x in 0..nw {
            let x0 = (x * 2).min(width - 1);
            let x1 = (x * 2 + 1).min(width - 1);
            let y0 = (y * 2).min(height - 1);
            let y1 = (y * 2 + 1).min(height - 1);
            for c in 0..4usize {
                let sum = rgba[((y0 * width + x0) * 4) as usize + c] as u32
                    + rgba[((y0 * width + x1) * 4) as usize + c] as u32
                    + rgba[((y1 * width + x0) * 4) as usize + c] as u32
                    + rgba[((y1 * width + x1) * 4) as usize + c] as u32;
                out[((y * nw + x) * 4) as usize + c] = (sum / 4) as u8;
            }
        }
    }
    (nw, nh, out)
}

fn build_mip_chain(rgba: &[u8], width: u32, height: u32) -> Vec<(u32, u32, Vec<u8>)> {
    let mut levels = vec![(width, height, rgba.to_vec())];
    loop {
        let &(w, h, ref data) = levels.last().unwrap();
        if w == 1 && h == 1 {
            break;
        }
        levels.push(downsample_half(data, w, h));
    }
    levels
}

pub fn encode(width: u32, height: u32, rgba: &[u8], format: D3DFormat) -> Result<Vec<u8>> {
    if rgba.len() != (width as usize) * (height as usize) * 4 {
        bail!(
            "rgba buffer length {} does not match {}x{} RGBA8",
            rgba.len(),
            width,
            height
        );
    }

    let levels = build_mip_chain(rgba, width, height);

    let mut dds = Dds::new_d3d(NewD3dParams {
        height,
        width,
        depth: None,
        format,
        mipmap_levels: Some(levels.len() as u32),
        caps2: None,
    })?;

    match format {
        D3DFormat::L8 => {
            dds.header.spf = PixelFormat {
                size: 32,
                flags: PixelFormatFlags::LUMINANCE,
                fourcc: None,
                rgb_bit_count: Some(8),
                r_bit_mask: Some(0xff),
                g_bit_mask: None,
                b_bit_mask: None,
                a_bit_mask: None,
            };
        }
        D3DFormat::A8 => {
            dds.header.spf = PixelFormat {
                size: 32,
                flags: PixelFormatFlags::ALPHA,
                fourcc: None,
                rgb_bit_count: None,
                r_bit_mask: None,
                g_bit_mask: None,
                b_bit_mask: None,
                a_bit_mask: Some(0xff),
            };
        }
        _ => {}
    }

    let mut body = Vec::new();
    for (lw, lh, ldata) in &levels {
        if let Some(bc) = bc_format_for(format) {
            let mut level_body = vec![0u8; bc.compressed_size(*lw as usize, *lh as usize)];
            bc.compress(ldata, *lw as usize, *lh as usize, BcParams::default(), &mut level_body);
            body.extend_from_slice(&level_body);
        } else {
            body.extend_from_slice(&pack_uncompressed(ldata, format)?);
        }
    }

    if body.len() > dds.data.len() {
        bail!(
            "encoded body ({} bytes) exceeds allocated DDS data buffer ({} bytes)",
            body.len(),
            dds.data.len()
        );
    }
    dds.data[..body.len()].copy_from_slice(&body);

    let mut out = Vec::new();
    dds.write(&mut out)?;
    Ok(out)
}

