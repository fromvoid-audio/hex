#include "hex_device_abi_v1.h"

#include <math.h>
#include <stdlib.h>
#include <string.h>

#define TWO_PI 6.283185307179586476925286766559
#define PARAM_AMOUNT 1u
#define PARAM_RATE_HZ 2u
#define PARAM_PHASE_DEG 3u
#define PARAM_SHAPE 4u
#define PARAM_INVERT 5u

typedef struct AutoPanState {
    uint32_t sample_rate;
    float amount;
    float rate_hz;
    float phase_deg;
    float shape;
    float invert;
    double lfo_phase;
} AutoPanState;

static void write_str(uint8_t* dst, size_t cap, const char* src, uint32_t* out_len) {
    size_t len = strlen(src);
    if (len > cap) len = cap;
    memset(dst, 0, cap);
    memcpy(dst, src, len);
    *out_len = (uint32_t)len;
}

static int32_t ap_describe(HexAbiDescriptor* out) {
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
    out->tail_frames = 0u;
    out->parameter_count = 5u;
    out->max_state_bytes = (uint32_t)sizeof(AutoPanState);
    write_str(out->type_id, HEX_PACKAGE_ABI_MAX_TYPE_ID_BYTES, "hex.pkg.auto_pan", &out->type_id_len);
    return HEX_ABI_OK;
}

static int32_t ap_param_desc(uint32_t index, HexAbiParameterDescriptor* out) {
    if (!out || index >= 5u) return HEX_ABI_INVALID_ARG;
    memset(out, 0, sizeof(*out));
    out->struct_size = (uint32_t)sizeof(HexAbiParameterDescriptor);
    switch (index) {
        case 0u:
            out->id = PARAM_AMOUNT; out->min = 0.0f; out->max = 1.0f; out->default_value = 0.5f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "amount", &out->name_len);
            break;
        case 1u:
            out->id = PARAM_RATE_HZ; out->min = 0.05f; out->max = 20.0f; out->default_value = 2.0f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "rate", &out->name_len);
            break;
        case 2u:
            out->id = PARAM_PHASE_DEG; out->min = 0.0f; out->max = 360.0f; out->default_value = 180.0f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "phase", &out->name_len);
            break;
        case 3u:
            out->id = PARAM_SHAPE; out->min = 0.0f; out->max = 1.0f; out->default_value = 0.0f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "shape", &out->name_len);
            break;
        case 4u:
            out->id = PARAM_INVERT; out->min = 0.0f; out->max = 1.0f; out->default_value = 0.0f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "invert", &out->name_len);
            break;
        default:
            return HEX_ABI_INVALID_ARG;
    }
    return HEX_ABI_OK;
}

static int32_t ap_create(uint32_t sample_rate, void** out_instance) {
    if (!out_instance || sample_rate < 8000u || sample_rate > 384000u) return HEX_ABI_INVALID_RATE;
    AutoPanState* st = (AutoPanState*)calloc(1, sizeof(AutoPanState));
    if (!st) return HEX_ABI_ALLOCATION_FAILED;
    st->sample_rate = sample_rate;
    st->amount = 0.5f;
    st->rate_hz = 2.0f;
    st->phase_deg = 180.0f;
    st->shape = 0.0f;
    st->invert = 0.0f;
    st->lfo_phase = 0.0;
    *out_instance = st;
    return HEX_ABI_OK;
}

static void ap_destroy(void* instance) { free(instance); }

static int32_t ap_set_state(void* instance, const uint8_t* bytes, uint32_t len) {
    if (!instance || !bytes || len != sizeof(AutoPanState)) return HEX_ABI_INVALID_STATE;
    AutoPanState cand;
    memcpy(&cand, bytes, sizeof(AutoPanState));
    if (!isfinite(cand.amount) || cand.amount < 0.0f || cand.amount > 1.0f ||
        !isfinite(cand.rate_hz) || cand.rate_hz < 0.05f || cand.rate_hz > 20.0f ||
        !isfinite(cand.phase_deg) || cand.phase_deg < 0.0f || cand.phase_deg > 360.0f ||
        !isfinite(cand.shape) || cand.shape < 0.0f || cand.shape > 1.0f ||
        !isfinite(cand.invert) || !isfinite(cand.lfo_phase)) {
        return HEX_ABI_INVALID_STATE;
    }
    uint32_t sr = ((AutoPanState*)instance)->sample_rate;
    memcpy(instance, &cand, sizeof(AutoPanState));
    ((AutoPanState*)instance)->sample_rate = sr;
    return HEX_ABI_OK;
}

static int32_t ap_get_state(const void* instance, uint8_t* out_bytes, uint32_t capacity, uint32_t* out_len) {
    if (!instance || !out_len) return HEX_ABI_INVALID_ARG;
    *out_len = (uint32_t)sizeof(AutoPanState);
    if (!out_bytes || capacity < sizeof(AutoPanState)) return HEX_ABI_SIZE_MISMATCH;
    memcpy(out_bytes, instance, sizeof(AutoPanState));
    return HEX_ABI_OK;
}

static float eval_lfo(double phase, float shape) {
    float s = (float)sin(phase);
    if (shape <= 0.0f) return s;
    float sq = s >= 0.0f ? 1.0f : -1.0f;
    return s + (sq - s) * shape;
}

static int32_t ap_process(
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

    AutoPanState* st = (AutoPanState*)instance;
    double sr = (double)st->sample_rate;
    uint32_t ev_idx = 0u;
    for (uint32_t i = 0u; i < ctx->frames; ++i) {
        while (param_events && ev_idx < ctx->param_event_count && param_events[ev_idx].sample_offset <= i) {
            uint32_t id = param_events[ev_idx].parameter_id;
            float v = param_events[ev_idx].value;
            if (isfinite(v)) {
                if (id == PARAM_AMOUNT && v >= 0.0f && v <= 1.0f) st->amount = v;
                else if (id == PARAM_RATE_HZ && v >= 0.05f && v <= 20.0f) st->rate_hz = v;
                else if (id == PARAM_PHASE_DEG && v >= 0.0f && v <= 360.0f) st->phase_deg = v;
                else if (id == PARAM_SHAPE && v >= 0.0f && v <= 1.0f) st->shape = v;
                else if (id == PARAM_INVERT) st->invert = v >= 0.5f ? 1.0f : 0.0f;
            }
            ++ev_idx;
        }
        double offset_rad = ((double)st->phase_deg / 360.0) * TWO_PI;
        float sign = st->invert >= 0.5f ? -1.0f : 1.0f;
        float mod_l = 0.5f * (1.0f + sign * eval_lfo(st->lfo_phase, st->shape));
        float mod_r = 0.5f * (1.0f + sign * eval_lfo(st->lfo_phase + offset_rad, st->shape));
        float gain_l = 1.0f - st->amount * (1.0f - mod_l);
        float gain_r = 1.0f - st->amount * (1.0f - mod_r);
        out_stereo[2u * i] = in_stereo[2u * i] * gain_l;
        out_stereo[2u * i + 1u] = in_stereo[2u * i + 1u] * gain_r;
        st->lfo_phase += (TWO_PI * (double)st->rate_hz) / sr;
        if (st->lfo_phase >= TWO_PI) {
            st->lfo_phase -= TWO_PI;
        }
    }
    return HEX_ABI_OK;
}

int32_t hex_device_abi_v1_entry(uint32_t requested_abi_version, HexAbiVtableV1* out_vtable) {
    if (!out_vtable) return HEX_ABI_INVALID_ARG;
    if (requested_abi_version != HEX_PACKAGE_ABI_VERSION_V1) return HEX_ABI_UNSUPPORTED_ABI;
    if (out_vtable->struct_size != sizeof(HexAbiVtableV1)) return HEX_ABI_SIZE_MISMATCH;
    out_vtable->abi_version = HEX_PACKAGE_ABI_VERSION_V1;
    out_vtable->describe = ap_describe;
    out_vtable->parameter_descriptor = ap_param_desc;
    out_vtable->create = ap_create;
    out_vtable->destroy = ap_destroy;
    out_vtable->set_state = ap_set_state;
    out_vtable->get_state = ap_get_state;
    out_vtable->process = ap_process;
    return HEX_ABI_OK;
}
