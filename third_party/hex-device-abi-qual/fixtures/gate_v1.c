#include "hex_device_abi_v1.h"

#include <math.h>
#include <stdlib.h>
#include <string.h>

#define PARAM_THRESHOLD 1u
#define PARAM_FLOOR 2u
#define PARAM_ATTACK_MS 3u
#define PARAM_HOLD_MS 4u
#define PARAM_RELEASE_MS 5u
#define PARAM_FLIP 6u

typedef struct GateState {
    uint32_t sample_rate;
    float threshold_db;
    float floor_db;
    float attack_ms;
    float hold_ms;
    float release_ms;
    float flip;
    float gate_env;
    uint32_t hold_remaining;
} GateState;

static void write_str(uint8_t* dst, size_t cap, const char* src, uint32_t* out_len) {
    size_t len = strlen(src);
    if (len > cap) len = cap;
    memset(dst, 0, cap);
    memcpy(dst, src, len);
    *out_len = (uint32_t)len;
}

static int32_t gate_describe(HexAbiDescriptor* out) {
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
    out->parameter_count = 6u;
    out->max_state_bytes = (uint32_t)sizeof(GateState);
    write_str(out->type_id, HEX_PACKAGE_ABI_MAX_TYPE_ID_BYTES, "hex.pkg.gate", &out->type_id_len);
    return HEX_ABI_OK;
}

static int32_t gate_param_desc(uint32_t index, HexAbiParameterDescriptor* out) {
    if (!out || index >= 6u) return HEX_ABI_INVALID_ARG;
    memset(out, 0, sizeof(*out));
    out->struct_size = (uint32_t)sizeof(HexAbiParameterDescriptor);
    switch (index) {
        case 0u:
            out->id = PARAM_THRESHOLD; out->min = -80.0f; out->max = 0.0f; out->default_value = -24.0f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "threshold", &out->name_len);
            break;
        case 1u:
            out->id = PARAM_FLOOR; out->min = -80.0f; out->max = 0.0f; out->default_value = -60.0f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "floor", &out->name_len);
            break;
        case 2u:
            out->id = PARAM_ATTACK_MS; out->min = 0.1f; out->max = 100.0f; out->default_value = 2.0f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "attack", &out->name_len);
            break;
        case 3u:
            out->id = PARAM_HOLD_MS; out->min = 0.0f; out->max = 500.0f; out->default_value = 20.0f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "hold", &out->name_len);
            break;
        case 4u:
            out->id = PARAM_RELEASE_MS; out->min = 5.0f; out->max = 1000.0f; out->default_value = 100.0f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "release", &out->name_len);
            break;
        case 5u:
            out->id = PARAM_FLIP; out->min = 0.0f; out->max = 1.0f; out->default_value = 0.0f;
            write_str(out->name, HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES, "flip", &out->name_len);
            break;
        default:
            return HEX_ABI_INVALID_ARG;
    }
    return HEX_ABI_OK;
}

static int32_t gate_create(uint32_t sample_rate, void** out_instance) {
    if (!out_instance || sample_rate < 8000u || sample_rate > 384000u) return HEX_ABI_INVALID_RATE;
    GateState* st = (GateState*)calloc(1, sizeof(GateState));
    if (!st) return HEX_ABI_ALLOCATION_FAILED;
    st->sample_rate = sample_rate;
    st->threshold_db = -24.0f;
    st->floor_db = -60.0f;
    st->attack_ms = 2.0f;
    st->hold_ms = 20.0f;
    st->release_ms = 100.0f;
    st->flip = 0.0f;
    st->gate_env = 0.0f;
    st->hold_remaining = 0u;
    *out_instance = st;
    return HEX_ABI_OK;
}

static void gate_destroy(void* instance) { free(instance); }

static int32_t gate_set_state(void* instance, const uint8_t* bytes, uint32_t len) {
    if (!instance || !bytes || len != sizeof(GateState)) return HEX_ABI_INVALID_STATE;
    GateState cand;
    memcpy(&cand, bytes, sizeof(GateState));
    if (!isfinite(cand.threshold_db) || cand.threshold_db < -80.0f || cand.threshold_db > 0.0f ||
        !isfinite(cand.floor_db) || cand.floor_db < -80.0f || cand.floor_db > 0.0f ||
        !isfinite(cand.attack_ms) || cand.attack_ms < 0.1f || cand.attack_ms > 100.0f ||
        !isfinite(cand.hold_ms) || cand.hold_ms < 0.0f || cand.hold_ms > 500.0f ||
        !isfinite(cand.release_ms) || cand.release_ms < 5.0f || cand.release_ms > 1000.0f ||
        !isfinite(cand.flip) || !isfinite(cand.gate_env) || cand.gate_env < 0.0f || cand.gate_env > 1.0f) {
        return HEX_ABI_INVALID_STATE;
    }
    uint32_t sr = ((GateState*)instance)->sample_rate;
    memcpy(instance, &cand, sizeof(GateState));
    ((GateState*)instance)->sample_rate = sr;
    return HEX_ABI_OK;
}

static int32_t gate_get_state(const void* instance, uint8_t* out_bytes, uint32_t capacity, uint32_t* out_len) {
    if (!instance || !out_len) return HEX_ABI_INVALID_ARG;
    *out_len = (uint32_t)sizeof(GateState);
    if (!out_bytes || capacity < sizeof(GateState)) return HEX_ABI_SIZE_MISMATCH;
    memcpy(out_bytes, instance, sizeof(GateState));
    return HEX_ABI_OK;
}

static int32_t gate_process(
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

    GateState* st = (GateState*)instance;
    float sr = (float)st->sample_rate;
    uint32_t ev_idx = 0u;
    for (uint32_t i = 0u; i < ctx->frames; ++i) {
        while (param_events && ev_idx < ctx->param_event_count && param_events[ev_idx].sample_offset <= i) {
            uint32_t id = param_events[ev_idx].parameter_id;
            float v = param_events[ev_idx].value;
            if (isfinite(v)) {
                if (id == PARAM_THRESHOLD && v >= -80.0f && v <= 0.0f) st->threshold_db = v;
                else if (id == PARAM_FLOOR && v >= -80.0f && v <= 0.0f) st->floor_db = v;
                else if (id == PARAM_ATTACK_MS && v >= 0.1f && v <= 100.0f) st->attack_ms = v;
                else if (id == PARAM_HOLD_MS && v >= 0.0f && v <= 500.0f) st->hold_ms = v;
                else if (id == PARAM_RELEASE_MS && v >= 5.0f && v <= 1000.0f) st->release_ms = v;
                else if (id == PARAM_FLIP) st->flip = v >= 0.5f ? 1.0f : 0.0f;
            }
            ++ev_idx;
        }
        float l = in_stereo[2u * i];
        float r = in_stereo[2u * i + 1u];
        float peak = fmaxf(fabsf(l), fabsf(r));
        float thresh_lin = powf(10.0f, st->threshold_db * 0.05f);
        float open_target = 0.0f;
        if (peak >= thresh_lin) {
            st->hold_remaining = (uint32_t)(st->hold_ms * 0.001f * sr);
            open_target = 1.0f;
        } else if (st->hold_remaining > 0u) {
            --st->hold_remaining;
            open_target = 1.0f;
        }
        float atk = expf(-1.0f / fmaxf(1.0f, st->attack_ms * 0.001f * sr));
        float rel = expf(-1.0f / fmaxf(1.0f, st->release_ms * 0.001f * sr));
        float coeff = open_target > st->gate_env ? atk : rel;
        st->gate_env = coeff * st->gate_env + (1.0f - coeff) * open_target;

        float active = st->flip >= 0.5f ? (1.0f - st->gate_env) : st->gate_env;
        float floor_lin = powf(10.0f, st->floor_db * 0.05f);
        float gain = floor_lin + (1.0f - floor_lin) * active;
        out_stereo[2u * i] = l * gain;
        out_stereo[2u * i + 1u] = r * gain;
    }
    return HEX_ABI_OK;
}

int32_t hex_device_abi_v1_entry(uint32_t requested_abi_version, HexAbiVtableV1* out_vtable) {
    if (!out_vtable) return HEX_ABI_INVALID_ARG;
    if (requested_abi_version != HEX_PACKAGE_ABI_VERSION_V1) return HEX_ABI_UNSUPPORTED_ABI;
    if (out_vtable->struct_size != sizeof(HexAbiVtableV1)) return HEX_ABI_SIZE_MISMATCH;
    out_vtable->abi_version = HEX_PACKAGE_ABI_VERSION_V1;
    out_vtable->describe = gate_describe;
    out_vtable->parameter_descriptor = gate_param_desc;
    out_vtable->create = gate_create;
    out_vtable->destroy = gate_destroy;
    out_vtable->set_state = gate_set_state;
    out_vtable->get_state = gate_get_state;
    out_vtable->process = gate_process;
    return HEX_ABI_OK;
}
