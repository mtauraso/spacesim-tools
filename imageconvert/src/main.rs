
#[macro_use]
extern crate bmp;
extern crate dataview;
use bmp::{Image, Pixel};
use std::{env, fs, iter::zip, path::Path};
use clap::Parser;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Path to the .PLT file with the custom palette for the image. 
    /// If only a palette is provided, it will be converted to a bitmap.
    #[arg(short, long)]
    palette_path: Option<std::path::PathBuf>,

    /// Path to .R8 file to convert. If an R8 file is given without a palette
    /// the top of the palette will be set to RGB(0,255,0) to visually flag 
    /// issues.
    #[arg(short, long)]
    image_path: Option<std::path::PathBuf>,

    /// Turns on debug mode, which will output
    /// two palette bmps. DEBUG_PLT_6.BMP and DEBUG_PLT_8.bmp)
    /// containing 6-bit and 8-bit RGB values used
    #[arg(short, long, default_value_t = false)]
    debug: bool,
}

fn main() {
    let args = Args::parse();

    match args.image_path {
        Some(image) => image_to_bitmap(&image, args.palette_path.as_deref(), args.debug),
        None => match args.palette_path {
                Some(palette) => palette_file_to_bitmap(palette.as_path()),
                None => {println!("Must provide either a palette or image or both.")},
        }
    };
}

#[derive(Clone)]
#[derive(dataview::Pod)]
#[repr(C)]
struct PalettePixel {
    r: u8,
    g: u8,
    b: u8,
}

impl PalettePixel {
    /// Left shift a 6 bit color value, using the top two bits
    /// as the bottom two bits of the new value
    ///
    ///  0xC0 == 0b00110000
    ///
    /// This means that 
    ///    - Black is preserved: 0b00000000 -> 0b00000000
    ///    - White is preserved: 0b00111111 -> 0b11111111
    ///    - Intermediate values interpolate smoothly:
    ///        (0x0E) 0b00001110 -> 0b00111000 (0x38)
    ///        (0x0F) 0b00001111 -> 0b00111100 (0x3C == 0x38 + 4)
    ///        (0x10) 0b00010000 -> 0b01000001 (0x41 == 0x3C + 5)
    ///        (0x11) 0b00010001 -> 0b01000101 (0x45 == 0x41 + 4)
    fn to_8bit_val(pixel: u8) -> u8 {

        let lowbits: u8 = (pixel & 0x30) >> 4;
        (pixel << 2) | lowbits
    }

    /// Reverse of .to_8bit_val for every output of .to_8bit_val
    /// Quantized for in-between values
    fn to_6bit_val(pixel:u8) -> u8 {
        pixel >> 2
    }
}
pub trait ConvertibleRGB {
    fn to_8bit(&mut self);
    fn to_6bit(&mut self);
}

impl ConvertibleRGB for PalettePixel {
    fn to_8bit(&mut self) {
        self.r = Self::to_8bit_val(self.r); 
        self.g = Self::to_8bit_val(self.g); 
        self.b = Self::to_8bit_val(self.b); 
    }

    fn to_6bit(&mut self) {
        self.r = Self::to_6bit_val(self.r); 
        self.g = Self::to_6bit_val(self.g); 
        self.b = Self::to_6bit_val(self.b); 
    }
}

/// These do not convert the first 16 colors in a Palette, because those pixels appear to be special
/// And retain their 6 bit values even when rendering in a modern 8 bit RGB context.
impl ConvertibleRGB for Vec<PalettePixel> {
    fn to_6bit(&mut self){
        for i in 16..self.len() {
            self[i].to_6bit();
        }
    }
    fn to_8bit(&mut self){
        for i in 16..self.len() {
            self[i].to_8bit();
        }
    }
}

#[derive(Clone)]
#[derive(dataview::Pod)]
#[repr(C)]
struct IndexPixel {
    raw_index: u8,
}

fn const_palette(size:usize, value: PalettePixel) -> Vec<PalettePixel> {
    vec![value; size]
}

fn overlay_palette(mut palette: Vec<PalettePixel>, overlay: Vec<PalettePixel>, offset:usize) -> Vec<PalettePixel> {
    for (pixel, overlay_pixel) in zip(palette.iter_mut().skip(offset), overlay) {
        *pixel = overlay_pixel
    }
    palette
}

fn load_palette(path: &Path) -> Vec<PalettePixel> {
    let palette_file_bytes = fs::read(path).expect("Could not read palette file");
    let palette_pixel_zero = PalettePixel { r: 0, g: 0, b: 0 };
    let mut palette_file_colors: Vec<PalettePixel> = vec![palette_pixel_zero; palette_file_bytes.len()/3];
    for (bytes, color) in zip(palette_file_bytes.chunks_exact(3), &mut palette_file_colors){
        let view = dataview::DataView::from_mut(color);
        view.write(0,bytes);
    }

    println!("Found {} colors in palette {}", palette_file_colors.len(), path.display());
    palette_file_colors
}

fn load_image(path: &Path) -> (Vec<IndexPixel>, Vec<u8>) {
    let image_file_bytes = fs::read(path).expect("Could not read image file.");
    let mut image_file_indexes: Vec<IndexPixel> = vec![IndexPixel{raw_index:0}; image_file_bytes.len()];

    for (byte, index) in zip(image_file_bytes.clone(), &mut image_file_indexes){
        dataview::DataView::from_mut(index).write(0, &byte);
    }
    (image_file_indexes, image_file_bytes)
}

/// Assemble a VGA Palette the way I believe SPACESIM.exe does it
/// VGA Colors as the background, SPACESIM color palette above that, and the per-image/sprite palette filling the top 128 bits
fn spacesim_palette(palette_path: Option<&Path>, debug: bool) -> Vec<PalettePixel> {
    // Make every color 0 to start.
    let mut palette = const_palette(256, PalettePixel { r: 0, g: 0, b: 0 });
    // Fill with default VGA colors
    palette = overlay_palette(palette, default_vga_palette(), 0);
    // Overlay the palette from the space simulator dump
    palette = overlay_palette(palette, simulator_dump_palette(), 32);

    // Add in our custom palette if provided
    // If not, fill the space with electric green to highlight issues.
    palette = match palette_path {
        None => overlay_palette(palette, const_palette(128, PalettePixel { r: 0, g: 255, b: 0 }),128),        
        Some(p_path) => overlay_palette(palette, load_palette(&p_path), 128),
    };

    //DEBUG: Identify a line of 24 colors by setting the value to bright green
    // let mut offset = 32 ;
    // let line = 2;
    // let line_length = 24;
    // offset = offset + line*line_length;
    // for i in offset..(offset+line_length){
    //     palette[i] = PalettePixel{r:0,g:255,b:0};
    // }
    
    // Color correct every value except the first 16 compatibility colors to an 8 bit representation
    // So they represent what would be visible on modern hardware.
    palette.to_8bit();

    if debug {
        save_palette(&palette, "IMAGECONVERT_DEBUG");
    }
    
    return palette;
}

fn image_to_bitmap(image_path: &Path, palette_path: Option<&Path>, debug: bool){
    println!("Attempting to open image {} using custom palette {}", 
        image_path.display(), 
        match palette_path {
            None => String::from("<No Custom Palette>"),
            Some(p) => p.display().to_string(),
        }
    );

    let (image_file_indexes, image_file_bytes) = load_image(image_path);

    let palette = spacesim_palette(palette_path, debug);

    // Hardcoded BPP, W, H for space simulator images.
    let bpp: u32 = 1; // Bits per pixel
    let width: u32 = 256; // Width (Pixels per row)
    let height: u32 = 256; // Height

    // Assert we have the right number of bytes
    assert_eq!(
        image_file_bytes.len(), 
        (bpp*width*height).try_into().unwrap(), 
        "Must supply a 65536 byte 256x256 SPACESIM .R8 image");

    let mut img = Image::new(width, height);

    for ((x,y), index) in zip(img.coordinates(), image_file_indexes) {
        let palette_index:usize = index.raw_index.into();
        let color = &palette[palette_index];
        let pixel = px!(color.r, color.g, color.b);
        //let pixel = px!(gamma_correct(color.r), gamma_correct(color.g), gamma_correct(color.b));
        img.set_pixel(x, y, pixel);
    }

    // Write out the image
    let path = env::current_dir().unwrap();
    println!("Output Directory: {}",path.display());
    let file_basename = image_path.file_stem().expect("Could not find Base Filename.");
    let out_filename = format!("{}_{}.BMP", file_basename.to_str().unwrap(), image_path.extension().unwrap().to_str().unwrap());
    let outfile = Path::new(&out_filename);

    println!("Writing out {}", outfile.display());
    let _ = img.save(outfile);
    println!("Done!");
}

fn palette_file_to_bitmap(palette_path: &Path) {
    let mut palette = spacesim_palette(Some(palette_path), false);

    let path = env::current_dir().unwrap();
    println!("Output Directory: {}",path.display());
    let file_basename = palette_path.file_stem().expect("Could not find Base Filename.");

    save_palette(&mut palette, file_basename.to_str().unwrap());
}

fn save_palette(palette: &Vec<PalettePixel>, basename: &str) {
    // Output the 8 bit representation of our palette showing what the values would look like on the screen
    // For visual inspection.
    let out_filename_8 = format!("{}_PAL_8.BMP", basename);
    palette_to_bitmap(&palette, Path::new(&out_filename_8));

    let mut palette_6_bit = palette.clone();
    palette_6_bit.to_6bit();

    // Output the 6 bit representation of our palette indicating the values in memory that would be
    // in use in the VGA DAC Palette.
    let out_filename_6 = format!("{}_PAL_6.BMP", basename);
    palette_to_bitmap(&palette_6_bit, Path::new(&out_filename_6));
}

fn palette_to_bitmap(palette: &Vec<PalettePixel>, save_path:&Path) {    
    let palette_size = palette.len();

    let box_cols = 16usize;
    let mut box_rows = palette_size/box_cols;
    if box_cols*box_rows < palette_size {
        box_rows += 1;
    }

    let box_size_px = 16usize;
    let box_border_px= 1usize;
    let width:u32 = (box_cols*(box_size_px + box_border_px) + box_border_px).try_into().unwrap();
    let height:u32 = (box_rows*(box_size_px + box_border_px) + box_border_px).try_into().unwrap();

    let mut img = Image::new(width, height);

    for (x,y) in img.coordinates() {
        img.set_pixel(x,y, px!(0,0,0))
    }

    for i in 0..box_rows{
        let ymin = box_border_px * (i+1) + box_size_px * i;
        for j in 0.. box_cols{
            let xmin = box_border_px * (j+1) + box_size_px * j;
            let pixel_index = i*box_cols + j;
            if pixel_index < palette_size {
                let palette_color = &palette[pixel_index];
                let image_pixel = px!(palette_color.r, palette_color.g, palette_color.b);
                draw_box(&mut img, xmin, ymin, box_size_px, image_pixel)
            }
        }
    }

    println!("Writing out Palette {}", save_path.display());
    let _ = img.save(save_path);
}

fn draw_box(img: &mut Image, x:usize, y:usize, s:usize, color:Pixel) {
    let side_length: u32 = s.try_into().unwrap();
    let xmin:u32 = x.try_into().unwrap();
    let ymin:u32 = y.try_into().unwrap();
    let xmax:u32 = xmin + side_length;
    let ymax:u32 = ymin + side_length;
    for (x,y) in img.coordinates() {
        if x >= xmin && x < xmax && y >= ymin && y < ymax {
            img.set_pixel(x,y, color)
        }
    }
}

/// This is the default 6-bit RGB VGA DAC Palette
/// Used to initialize colors.
fn default_vga_palette() -> Vec<PalettePixel> {
    struct Color(u8,u8,u8);
    let colors= vec![
        // Compatibility
        Color(0x00,0x00,0x00),Color(0x00,0x00,0x2a),Color(0x00,0x2a,0x00),Color(0x00,0x2a,0x2a),Color(0x2a,0x00,0x00),Color(0x2a,0x00,0x2a),Color(0x2a,0x15,0x00),Color(0x2a,0x2a,0x2a),
        Color(0x15,0x15,0x15),Color(0x15,0x15,0x3f),Color(0x15,0x3f,0x15),Color(0x15,0x3f,0x3f),Color(0x3f,0x15,0x15),Color(0x3f,0x15,0x3f),Color(0x3f,0x3f,0x15),Color(0x3f,0x3f,0x3f),
        
        // Greyscale
        Color(0x00,0x00,0x00),Color(0x05,0x05,0x05),Color(0x08,0x08,0x08),Color(0x0b,0x0b,0x0b),Color(0x0e,0x0e,0x0e),Color(0x11,0x11,0x11),Color(0x14,0x14,0x14),Color(0x18,0x18,0x18),
        Color(0x1c,0x1c,0x1c),Color(0x20,0x20,0x20),Color(0x24,0x24,0x24),Color(0x28,0x28,0x28),Color(0x2d,0x2d,0x2d),Color(0x32,0x32,0x32),Color(0x38,0x38,0x38),Color(0x3f,0x3f,0x3f),

        // First block of 24x3
        Color(0x00,0x00,0x3f),Color(0x10,0x00,0x3f),Color(0x1f,0x00,0x3f),Color(0x2f,0x00,0x3f),Color(0x3f,0x00,0x3f),Color(0x3f,0x00,0x2f),Color(0x3f,0x00,0x1f),Color(0x3f,0x00,0x10),
        Color(0x3f,0x00,0x00),Color(0x3f,0x10,0x00),Color(0x3f,0x1f,0x00),Color(0x3f,0x2f,0x00),Color(0x3f,0x3f,0x00),Color(0x2f,0x3f,0x00),Color(0x1f,0x3f,0x00),Color(0x10,0x3f,0x00),
        Color(0x00,0x3f,0x00),Color(0x00,0x3f,0x10),Color(0x00,0x3f,0x1f),Color(0x00,0x3f,0x2f),Color(0x00,0x3f,0x3f),Color(0x00,0x2f,0x3f),Color(0x00,0x1f,0x3f),Color(0x00,0x10,0x3f),
        
        Color(0x1f,0x1f,0x3f),Color(0x27,0x1f,0x3f),Color(0x2f,0x1f,0x3f),Color(0x37,0x1f,0x3f),Color(0x3f,0x1f,0x3f),Color(0x3f,0x1f,0x37),Color(0x3f,0x1f,0x2f),Color(0x3f,0x1f,0x27),
        Color(0x3f,0x1f,0x1f),Color(0x3f,0x27,0x1f),Color(0x3f,0x2f,0x1f),Color(0x3f,0x37,0x1f),Color(0x3f,0x3f,0x1f),Color(0x37,0x3f,0x1f),Color(0x2f,0x3f,0x1f),Color(0x27,0x3f,0x1f),
        Color(0x1f,0x3f,0x1f),Color(0x1f,0x3f,0x27),Color(0x1f,0x3f,0x2f),Color(0x1f,0x3f,0x37),Color(0x1f,0x3f,0x3f),Color(0x1f,0x37,0x3f),Color(0x1f,0x2f,0x3f),Color(0x1f,0x27,0x3f),
        
        Color(0x2d,0x2d,0x3f),Color(0x31,0x2d,0x3f),Color(0x36,0x2d,0x3f),Color(0x3a,0x2d,0x3f),Color(0x3f,0x2d,0x3f),Color(0x3f,0x2d,0x3a),Color(0x3f,0x2d,0x36),Color(0x3f,0x2d,0x31),
        Color(0x3f,0x2d,0x2d),Color(0x3f,0x31,0x2d),Color(0x3f,0x36,0x2d),Color(0x3f,0x3a,0x2d),Color(0x3f,0x3f,0x2d),Color(0x3a,0x3f,0x2d),Color(0x36,0x3f,0x2d),Color(0x31,0x3f,0x2d),
        Color(0x2d,0x3f,0x2d),Color(0x2d,0x3f,0x31),Color(0x2d,0x3f,0x36),Color(0x2d,0x3f,0x3a),Color(0x2d,0x3f,0x3f),Color(0x2d,0x3a,0x3f),Color(0x2d,0x36,0x3f),Color(0x2d,0x31,0x3f),
        
        // Second block of 24x3
        Color(0x00,0x00,0x1c),Color(0x07,0x00,0x1c),Color(0x0e,0x00,0x1c),Color(0x15,0x00,0x1c),Color(0x1c,0x00,0x1c),Color(0x1c,0x00,0x15),Color(0x1c,0x00,0x0e),Color(0x1c,0x00,0x07),
        Color(0x1c,0x00,0x00),Color(0x1c,0x07,0x00),Color(0x1c,0x0e,0x00),Color(0x1c,0x15,0x00),Color(0x1c,0x1c,0x00),Color(0x15,0x1c,0x00),Color(0x0e,0x1c,0x00),Color(0x07,0x1c,0x00),
        Color(0x00,0x1c,0x00),Color(0x00,0x1c,0x07),Color(0x00,0x1c,0x0e),Color(0x00,0x1c,0x15),Color(0x00,0x1c,0x1c),Color(0x00,0x15,0x1c),Color(0x00,0x0e,0x1c),Color(0x00,0x07,0x1c),

        Color(0x0e,0x0e,0x1c),Color(0x11,0x0e,0x1c),Color(0x15,0x0e,0x1c),Color(0x18,0x0e,0x1c),Color(0x1c,0x0e,0x1c),Color(0x1c,0x0e,0x18),Color(0x1c,0x0e,0x15),Color(0x1c,0x0e,0x11),
        Color(0x1c,0x0e,0x0e),Color(0x1c,0x11,0x0e),Color(0x1c,0x15,0x0e),Color(0x1c,0x18,0x0e),Color(0x1c,0x1c,0x0e),Color(0x18,0x1c,0x0e),Color(0x15,0x1c,0x0e),Color(0x11,0x1c,0x0e),
        Color(0x0e,0x1c,0x0e),Color(0x0e,0x1c,0x11),Color(0x0e,0x1c,0x15),Color(0x0e,0x1c,0x18),Color(0x0e,0x1c,0x1c),Color(0x0e,0x18,0x1c),Color(0x0e,0x15,0x1c),Color(0x0e,0x11,0x1c),
        
        Color(0x14,0x14,0x1c),Color(0x16,0x14,0x1c),Color(0x18,0x14,0x1c),Color(0x1a,0x14,0x1c),Color(0x1c,0x14,0x1c),Color(0x1c,0x14,0x1a),Color(0x1c,0x14,0x18),Color(0x1c,0x14,0x16),
        Color(0x1c,0x14,0x14),Color(0x1c,0x16,0x14),Color(0x1c,0x18,0x14),Color(0x1c,0x1a,0x14),Color(0x1c,0x1c,0x14),Color(0x1a,0x1c,0x14),Color(0x18,0x1c,0x14),Color(0x16,0x1c,0x14),
        Color(0x14,0x1c,0x14),Color(0x14,0x1c,0x16),Color(0x14,0x1c,0x18),Color(0x14,0x1c,0x1a),Color(0x14,0x1c,0x1c),Color(0x14,0x1a,0x1c),Color(0x14,0x18,0x1c),Color(0x14,0x16,0x1c),
        
        // Third block of 24x3
        Color(0x00,0x00,0x10),Color(0x04,0x00,0x10),Color(0x08,0x00,0x10),Color(0x0c,0x00,0x10),Color(0x10,0x00,0x10),Color(0x10,0x00,0x0c),Color(0x10,0x00,0x08),Color(0x10,0x00,0x04),
        Color(0x10,0x00,0x00),Color(0x10,0x04,0x00),Color(0x10,0x08,0x00),Color(0x10,0x0c,0x00),Color(0x10,0x10,0x00),Color(0x0c,0x10,0x00),Color(0x08,0x10,0x00),Color(0x04,0x10,0x00),
        Color(0x00,0x10,0x00),Color(0x00,0x10,0x04),Color(0x00,0x10,0x08),Color(0x00,0x10,0x0c),Color(0x00,0x10,0x10),Color(0x00,0x0c,0x10),Color(0x00,0x08,0x10),Color(0x00,0x04,0x10),
        
        Color(0x08,0x08,0x10),Color(0x0a,0x08,0x10),Color(0x0c,0x08,0x10),Color(0x0e,0x08,0x10),Color(0x10,0x08,0x10),Color(0x10,0x08,0x0e),Color(0x10,0x08,0x0c),Color(0x10,0x08,0x0a),
        Color(0x10,0x08,0x08),Color(0x10,0x0a,0x08),Color(0x10,0x0c,0x08),Color(0x10,0x0e,0x08),Color(0x10,0x10,0x08),Color(0x0e,0x10,0x08),Color(0x0c,0x10,0x08),Color(0x0a,0x10,0x08),
        Color(0x08,0x10,0x08),Color(0x08,0x10,0x0a),Color(0x08,0x10,0x0c),Color(0x08,0x10,0x0e),Color(0x08,0x10,0x10),Color(0x08,0x0e,0x10),Color(0x08,0x0c,0x10),Color(0x08,0x0a,0x10),
        
        Color(0x0b,0x0b,0x10),Color(0x0c,0x0b,0x10),Color(0x0d,0x0b,0x10),Color(0x0f,0x0b,0x10),Color(0x10,0x0b,0x10),Color(0x10,0x0b,0x0f),Color(0x10,0x0b,0x0d),Color(0x10,0x0b,0x0c),
        Color(0x10,0x0b,0x0b),Color(0x10,0x0c,0x0b),Color(0x10,0x0d,0x0b),Color(0x10,0x0f,0x0b),Color(0x10,0x10,0x0b),Color(0x0f,0x10,0x0b),Color(0x0d,0x10,0x0b),Color(0x0c,0x10,0x0b),
        Color(0x0b,0x10,0x0b),Color(0x0b,0x10,0x0c),Color(0x0b,0x10,0x0d),Color(0x0b,0x10,0x0f),Color(0x0b,0x10,0x10),Color(0x0b,0x0f,0x10),Color(0x0b,0x0d,0x10),Color(0x0b,0x0c,0x10)
    ];
    let mut palette = vec![PalettePixel{r:0,g:0,b:0}; colors.len()];

    for (color, palette_pixel) in zip (colors, &mut palette) {
        *palette_pixel = PalettePixel{r: color.0, g: color.1, b: color.2};
    }
    palette
}


/// This is the first 128 bits of the VGA DAC palette dumped using the
/// DOSBOX-X debugger when space simulator is in the starting "FLIGHT" situation.
/// 
/// The first 32 colors (96 bytes) have been removed because they are the same
/// as the default VGA palette.
fn simulator_dump_palette () -> Vec<PalettePixel> {
    struct Color(u32);

    let colors = vec![
        // Reds
        Color(0x030101), Color(0x070202), Color(0x0b0303), Color(0x0f0404), Color(0x130505), Color(0x170606), Color(0x1b0707), Color(0x1f0808), 
        Color(0x230909), Color(0x270a0a), Color(0x2b0b0b), Color(0x2f0c0c), Color(0x330d0d), Color(0x370e0e), Color(0x3b0f0f), Color(0x3f1010), 
        
        // Oranges
        Color(0x030200), Color(0x070400), Color(0x0b0600), Color(0x0f0800), Color(0x130a00), Color(0x170c00), Color(0x1b0e00), Color(0x1f1000),
        Color(0x231200), Color(0x271400), Color(0x2b1600), Color(0x2f1800), Color(0x331a00), Color(0x371c00), Color(0x3b1e00), Color(0x3f2000),

        // Yellows
        Color(0x030200), Color(0x070600), Color(0x0b0a00), Color(0x0f0e00), Color(0x131200), Color(0x171600), Color(0x1b1a00), Color(0x1f1e00), 
        Color(0x232200), Color(0x272600), Color(0x2b2a00), Color(0x2f2e00), Color(0x333200), Color(0x373600), Color(0x3b3a00), Color(0x3f3e00),

        // Greens
        Color(0x000300), Color(0x010701), Color(0x020b02), Color(0x030f03), Color(0x041304), Color(0x051705), Color(0x061b06), Color(0x071f07), 
        Color(0x082308), Color(0x092709), Color(0x0a2b0a), Color(0x0b2f0b), Color(0x0c330c), Color(0x0d370d), Color(0x0e3b0e), Color(0x0f3f0f), 
        
        // Light blues
        Color(0x010203), Color(0x030507), Color(0x05080b), Color(0x070b0f), Color(0x090e13), Color(0x0b1117), Color(0x0d141b), Color(0x0f171f), 
        Color(0x111a23), Color(0x131d27), Color(0x15202b), Color(0x17232f), Color(0x192633), Color(0x1b2937), Color(0x1d2c3b), Color(0x1f2f3f),

        // Dark blues
        Color(0x000003), Color(0x000007), Color(0x00000b), Color(0x00000f), Color(0x000013), Color(0x000017), Color(0x00001b), Color(0x00001f), 
        Color(0x000023), Color(0x000027), Color(0x00002b), Color(0x00002f), Color(0x000033), Color(0x000037), Color(0x00003b), Color(0x00003f), 
    ];

    let mut palette = vec![PalettePixel{r:0,g:0,b:0}; colors.len()];

    for (color, palette_pixel) in zip (colors, &mut palette) {
        *palette_pixel = PalettePixel{
            r: ((color.0 & 0x00FF0000) >> 16).try_into().unwrap(), 
            g: ((color.0 & 0x0000FF00) >> 8).try_into().unwrap(), 
            b: (color.0 & 0x000000FF).try_into().unwrap(),
        };
    }
    palette
}

