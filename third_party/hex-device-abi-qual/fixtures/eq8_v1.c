#include "hex_device_abi_v1.h"

#include <math.h>
#include <stdlib.h>
#include <string.h>

#define TWO_PI 6.283185307179586476925286766559
#define EQ8_BANDS 8u

typedef struct Eq8Band {
    float freq_hz;
    float gain_db;
    float q;
    float enabled;
    float z1_l;
    float z2_l;
    float z1_r;
    float z2_r;
} Eq8Band;

typedef struct Eq8State {
    uint32_t sample_rate;
    Eq8Band bands[EQ8_BANDS];
} Eq8State;

static void write_str(uint8_t* dst, size_t cap, const char* src, uint32_t* out_len) {
    size_t len = strlen(src);
    if (len > cap) len = cap;
    memset(dst, 0, cap);
    memcpy(dst, src, len);
    *out_len = (uint32_t)len;
}

static int32_t eq8_describe(HexAbiDescriptor* out) {
    if (!out) return HEX_ABI_INVALID_ARG;
    memset(out, 0, sizeof(*out));
    out->struct_size = (uint32_t)sizeof(HexAbiDescriptor);
    out->magic = HEX_PACKAGE_ABI_MAGIC;
    out->abi_version = HEX_PACKAGE_ABI_VERSION_V1;
    out->device_kind = HEX_DEVICE_KIND_AUDIO_EFFECT;
    out->state_version = 1u;
    out->ui_version = 1u;
    out->package_major = 1u;
    out->package_minor = 0u;
    out->package_patch = 0u;
    out->capabilities = HEX_CAP_AUDIO_EFFECT | HEX_CAP_PARAMETER_EVENTS |
                        HEX_CAP_ZERO_LATENCY | HEX_CAP_STATE_PERSISTENCE;
    out->reported_latency_frames = 0u;
    out->tail_frames = 256u;
    out->parameter_count = EQ8_BANDS * 4u;
    out->max_state_bytes = (uint32_t)sizeof(Eq8State);
    write_str(out->type_id, HEX_PACKAGE_ABI_MAX_TYPE_ID_BYTES, "hex.pkg.eq8", &out->type_id_len);
    return HEX_ABI_OK;
}

static int32_t eq8_param_desc(uint32_t index, HexAbiParameterDescriptor* out) {
    if (!out || index >= EQ8_BANDS * 4u) return HEX_ABI_INVALID_ARG;
    static const float default_freqs[EQ8_BANDS] = {
        60.0f, 150.0f, 350.0f, 800.0f, 1600.0f, 3200.0f, 6400.0f, 12000.0f
    };
    uint32_t band = index / 4u;
    uint32_t field = index % 4u;
    memset(out, 0, sizeof(*out));
    out->struct_size = (uint32_t)sizeof(HexAbiParameterDescriptor);
    out->id = (band + 1u) * 10u + (field + 1u);
    if (field == 0u) {
        out->min = 20.0f; out->max = 20000.0f; out->default_value = default_freqs[band];
        write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "cutoff", &out->name_len);
    } else if (field == 1u) {
        out->min = -24.0f; out->max = 24.0f; out->default_value = 0.0f;
        write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "gain", &out->name_len);
    } else if (field == 2u) {
        out->min = 0.2f; out->max = 12.0f; out->default_value = 0.707f;
        write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "resonance", &out->name_len);
    } else {
        out->min = 0.0f; out->max = 1.0f; out->default_value = 1.0f;
        write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "amount", &out->name_len);
    }
    return HEX_ABI_OK;
}

static int32_t eq8_create(uint32_t sample_rate, void** out_instance) {
    if (!out_instance || sample_rate < 8000u || sample_rate > 384000u) return HEX_ABI_INVALID_RATE;
    static const float default_freqs[EQ8_BANDS] = {
        60.0f, 150.0f, 350.0f, 800.0f, 1600.0f, 3200.0f, 6400.0f, 12000.0f
    };
    Eq8State* st = (Eq8State*)calloc(1, sizeof(Eq8State));
    if (!st) return HEX_ABI_ALLOCATION_FAILED;
    st->sample_rate = sample_rate;
    for (uint32_t b = 0u; b < EQ8_BANDS; ++b) {
        st->bands[b].freq_hz = default_freqs[b];
        st->bands[b].gain_db = 0.0f;
        st->bands[b].q = 0.707f;
        st->bands[b].enabled = 1.0f;
    }
    *out_instance = st;
    return HEX_ABI_OK;
}

static void eq8_destroy(void* instance) { free(instance); }

static int32_t eq8_set_state(void* instance, const uint8_t* bytes, uint32_t len) {
    if (!instance || !bytes || len != sizeof(Eq8State)) return HEX_ABI_INVALID_STATE;
    Eq8State cand;
    memcpy(&cand, bytes, sizeof(Eq8State));
    for (uint32_t b = 0u; b < EQ8_BANDS; ++b) {
        Eq8Band* bb = &cand.bands[b];
        if (!isfinite(bb->freq_hz) || bb->freq_hz < 20.0f || bb->freq_hz > 20000.0f ||
            !isfinite(bb->gain_db) || bb->gain_db < -24.0f || bb->gain_db > 24.0f ||
            !isfinite(bb->q) || bb->q < 0.2f || bb->q > 12.0f ||
            !isfinite(bb->enabled) || !isfinite(bb->z1_l) || !isfinite(bb->z2_l) ||
            !isfinite(bb->z1_r) || !isfinite(bb->z2_r)) {
            return HEX_ABI_INVALID_STATE;
        }
    }
    uint32_t sr = ((Eq8State*)instance)->sample_rate;
    memcpy(instance, &cand, sizeof(Eq8State));
    ((Eq8State*)instance)->sample_rate = sr;
    return HEX_ABI_OK;
}

static int32_t eq8_get_state(const void* instance, uint8_t* out_bytes, uint32_t capacity, uint32_t* out_len) {
    if (!instance || !out_len) return HEX_ABI_INVALID_ARG;
    *out_len = (uint32_t)sizeof(Eq8State);
    if (!out_bytes || capacity < sizeof(Eq8State)) return HEX_ABI_SIZE_MISMATCH;
    memcpy(out_bytes, instance, sizeof(Eq8State));
    return HEX_ABI_OK;
}

static void peaking_coeffs(double sr, float freq_hz, float gain_db, float q,
                           float* b0, float* b1, float* b2, float* a1, float* a2) {
    double f = (double)freq_hz;
    if (f > sr * 0.45) f = sr * 0.45;
    double a = pow(10.0, (double)gain_db / 40.0);
    double w0 = (TWO_PI * f) / sr;
    double alpha = sin(w0) / (2.0 * (double)q);
    double cs = cos(w0);
    double inv_a0 = 1.0 / (1.0 + alpha / a);
    *b0 = (float)((1.0 + alpha * a) * inv_a0);
    *b1 = (float)((-2.0 * cs) * inv_a0);
    *b2 = (float)((1.0 - alpha * a) * inv_a0);
    *a1 = (float)((-2.0 * cs) * inv_a0);
    *a2 = (float)((1.0 - alpha / a) * inv_a0);
}

static int32_t eq8_process(
    void* instance,
    const HexAbiProcessContext* ctx,
    const float* in_stereo,
    float* out_stereo,
    const HexAbiParamEvent* param_events,
    const HexAbiNoteEvent* note_events
) {
    (void)note_events;
    if (!instance || !ctx || !in_stereo || !out_stereo) return HEX_ABI_INVALID_ARG;
    if (ctx->struct_size != sizeof(HexAbiProcessContext)) return HEX_ABI_SIZE_MISMATCH;
    if (ctx->abi_version != HEX_PACKAGE_ABI_VERSION_V1) return HEX_ABI_UNSUPPORTED_ABI;
    if (ctx->frames == 0u || ctx->frames > HEX_PACKAGE_ABI_MAX_FRAMES) return HEX_ABI_INVALID_FRAMES;

    Eq8State* st = (Eq8State*)instance;
    double sr = (double)st->sample_rate;
    uint32_t ev_idx = 0u;
    for (uint32_t i = 0u; i < ctx->frames; ++i) {
        while (param_events && ev_idx < ctx->param_event_count && param_events[ev_idx].sample_offset <= i) {
            uint32_t id = param_events[ev_idx].parameter_id;
            float v = param_events[ev_idx].value;
            if (isfinite(v) && id >= 11u) {
                uint32_t band = (id / 10u) - 1u;
                uint32_t field = (id % 10u);
                if (band < EQ8_BANDS) {
                    if (field == 1u && v >= 20.0f && v <= 20000.0f) st->bands[band].freq_hz = v;
                    else if (field == 2u && v >= -24.0f && v <= 24.0f) st->bands[band].gain_db = v;
                    else if (field == 3u && v >= 0.2f && v <= 12.0f) st->bands[band].q = v;
                    else if (field == 4u) st->bands[band].enabled = v >= 0.5f ? 1.0f : 0.0f;
                }
            }
            ++ev_idx;
        }
        float l = in_stereo[2u * i];
        float r = in_stereo[2u * i + 1u];
        for (uint32_t b = 0u; b < EQ8_BANDS; ++b) {
            Eq8Band* bb = &st->bands[b];
            if (bb->enabled < 0.5f) continue;
            float b0, b1, b2, a1, a2;
            peaking_coeffs(sr, bb->freq_hz, bb->gain_db, bb->q, &b0, &b1, &b2, &a1, &a2);
            float yl = b0 * l + bb->z1_l;
            bb->z1_l = b1 * l - a1 * yl + bb->z2_l;
            bb->z2_l = b2 * l - a2 * yl;
            l = yl;
            float yr = b0 * r + bb->z1_r;
            bb->z1_r = b1 * r - a1 * yr + bb->z2_r;
            bb->z2_r = b2 * r - a2 * yr;
            r = yr;
        }
        out_stereo[2u * i] = l;
        out_stereo[2u * i + 1u] = r;
    }
    return HEX_ABI_OK;
}

int32_t hex_device_abi_v1_entry(uint32_t requested_abi_version, HexAbiVtableV1* out_vtable) {
    if (!out_vtable) return HEX_ABI_INVALID_ARG;
    if (requested_abi_version != HEX_PACKAGE_ABI_VERSION_V1) return HEX_ABI_UNSUPPORTED_ABI;
    if (out_vtable->struct_size != sizeof(HexAbiVtableV1)) return HEX_ABI_SIZE_MISMATCH;
    out_vtable->abi_version = HEX_PACKAGE_ABI_VERSION_V1;
    out_vtable->describe = eq8_describe;
    out_vtable->parameter_descriptor = eq8_param_desc;
    out_vtable->create = eq8_create;
    out_vtable->destroy = eq8_destroy;
    out_vtable->set_state = eq8_set_state;
    out_vtable->get_state = eq8_get_state;
    out_vtable->process = eq8_process;
    return HEX_ABI_OK;
}
