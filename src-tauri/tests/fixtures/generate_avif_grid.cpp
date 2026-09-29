// Synthetic grid/collection/high-depth fixtures using libheif directly, independently of
// ImageMagick's writer. Build/run via generate_avif.py --codec-prefix PREFIX.
#include <libheif/heif.h>
#include <cstdio>
#include <cstdlib>

static void check(heif_error error) {
  if (error.code != heif_error_Ok) {
    std::fprintf(stderr, "%s\n", error.message);
    std::exit(1);
  }
}

int main(int argc, char** argv) {
  if (argc != 4) return 2;
  check(heif_init(nullptr));
  heif_image* images[2];
  const unsigned char colors[2][3] = {{200, 10, 20}, {20, 30, 200}};
  for (int tile = 0; tile < 2; ++tile) {
    check(heif_image_create(64, 64, heif_colorspace_RGB,
                           heif_chroma_interleaved_RGB, &images[tile]));
    check(heif_image_add_plane(images[tile], heif_channel_interleaved, 64, 64, 8));
    int stride;
    auto pixels = heif_image_get_plane(images[tile], heif_channel_interleaved, &stride);
    for (int y = 0; y < 64; ++y)
      for (int x = 0; x < 64; ++x)
        for (int channel = 0; channel < 3; ++channel)
          pixels[y*stride + x*3 + channel] = colors[tile][channel];
  }
  auto nclx = heif_nclx_color_profile_alloc();
  nclx->color_primaries = heif_color_primaries_ITU_R_BT_709_5;
  nclx->transfer_characteristics = heif_transfer_characteristic_IEC_61966_2_1;
  nclx->matrix_coefficients = heif_matrix_coefficients_RGB_GBR;
  nclx->full_range_flag = 1;
  auto options = heif_encoding_options_alloc();
  options->output_nclx_profile = nclx;
  for (int mode = 0; mode < 2; ++mode) {
    auto context = heif_context_alloc();
    heif_encoder* encoder;
    check(heif_context_get_encoder_for_format(context, heif_compression_AV1, &encoder));
    check(heif_encoder_set_lossless(encoder, 1));
    check(heif_encoder_set_parameter(encoder, "chroma", "444"));
    check(heif_encoder_set_parameter_integer(encoder, "speed", 6));
    check(heif_encoder_set_parameter_integer(encoder, "threads", 2));
    if (mode == 0) {
      heif_image_handle* handle;
      // The 1.23.5 implementation takes columns, rows (its header names these
      // in the opposite order). Explicitly mark the derived AV1 image as AVIF.
      check(heif_context_encode_grid(context, images, 2, 1, encoder, options, &handle));
      heif_image_handle_release(handle);
      heif_context_set_major_brand(context, heif_brand2_avif);
      heif_context_add_compatible_brand(context, heif_brand2_avif);
    } else {
      for (auto image : images) {
        heif_image_handle* handle;
        check(heif_context_encode_image(context, image, encoder, options, &handle));
        heif_image_handle_release(handle);
      }
    }
    check(heif_context_write_to_file(context, argv[mode + 1]));
    heif_encoder_release(encoder);
    heif_context_free(context);
  }
  // Known 12-bit integer samples bypass ImageMagick's reader and writer, so its
  // sample-range expansion can be tested independently (especially opaque alpha).
  auto context = heif_context_alloc();
  heif_encoder* encoder;
  heif_image* high;
  check(heif_context_get_encoder_for_format(context, heif_compression_AV1, &encoder));
  check(heif_encoder_set_lossless(encoder, 1));
  check(heif_encoder_set_parameter(encoder, "chroma", "444"));
  check(heif_encoder_set_parameter_integer(encoder, "speed", 6));
  check(heif_encoder_set_parameter_integer(encoder, "threads", 2));
  check(heif_image_create(32, 20, heif_colorspace_RGB, heif_chroma_interleaved_RRGGBBAA_LE, &high));
  check(heif_image_add_plane(high, heif_channel_interleaved, 32, 20, 12));
  int stride;
  auto pixels = heif_image_get_plane(high, heif_channel_interleaved, &stride);
  const unsigned values[3] = {0, 2048, 4095};
  for (int y = 0; y < 20; ++y)
    for (int x = 0; x < 32; ++x)
      for (int channel = 0; channel < 4; ++channel) {
        unsigned value = values[x % 3];
        pixels[y*stride + x*8 + channel*2] = value & 255;
        pixels[y*stride + x*8 + channel*2 + 1] = value >> 8;
      }
  heif_image_handle* handle;
  check(heif_context_encode_image(context, high, encoder, options, &handle));
  check(heif_context_write_to_file(context, argv[3]));
  heif_image_handle_release(handle);
  heif_image_release(high);
  heif_encoder_release(encoder);
  heif_context_free(context);
  heif_encoding_options_free(options);
  heif_nclx_color_profile_free(nclx);
  for (auto image : images) heif_image_release(image);
  heif_deinit();
}
