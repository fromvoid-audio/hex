#ifndef HEX_DEVICE_ABI_V1_H
#define HEX_DEVICE_ABI_V1_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define HEX_PACKAGE_ABI_MAGIC 0x48455844u
#define HEX_PACKAGE_ABI_VERSION_V1 1u
#define HEX_PACKAGE_ABI_MAX_FRAMES 512u
#define HEX_PACKAGE_ABI_MAX_PARAMS 64u
#define HEX_PACKAGE_ABI_MAX_PARAM_EVENTS 64u
#define HEX_PACKAGE_ABI_MAX_NOTE_EVENTS 64u
#define HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES 64u
#define HEX_PACKAGE_ABI_MAX_STATE_BYTES 4096u
#define HEX_PACKAGE_ABI_MAX_TYPE_ID_BYTES 128u

#define HEX_DEVICE_KIND_AUDIO_EFFECT 1u
#define HEX_DEVICE_KIND_INSTRUMENT 2u

#define HEX_CAP_AUDIO_EFFECT (1ull << 0)
#define HEX_CAP_INSTRUMENT (1ull << 1)
#define HEX_CAP_PARAMETER_EVENTS (1ull << 2)
#define HEX_CAP_NOTE_EVENTS (1ull << 3)
#define HEX_CAP_ZERO_LATENCY (1ull << 4)
#define HEX_CAP_STATE_PERSISTENCE (1ull << 5)
#define HEX_CAP_NOTE_EXPRESSION (1ull << 6)
#define HEX_CAP_CHANNEL_EXPRESSION (1ull << 7)

#define HEX_NOTE_EVENT_ON 1u
#define HEX_NOTE_EVENT_OFF 2u
#define HEX_NOTE_EVENT_PITCH_BEND 3u
#define HEX_NOTE_EVENT_PRESSURE 4u
#define HEX_NOTE_EVENT_CC 5u
#define HEX_NOTE_EVENT_ALL_NOTES_OFF 6u

enum {
    HEX_ABI_OK = 0,
    HEX_ABI_INVALID_ARG = 1,
    HEX_ABI_UNSUPPORTED_ABI = 2,
    HEX_ABI_SIZE_MISMATCH = 3,
    HEX_ABI_INVALID_RATE = 4,
    HEX_ABI_INVALID_FRAMES = 5,
    HEX_ABI_INVALID_STATE = 6,
    HEX_ABI_INVALID_OUTPUT = 7,
    HEX_ABI_ALLOCATION_FAILED = 8,
    HEX_ABI_INTERNAL_FAULT = 9
};

typedef struct HexAbiDescriptor {
    uint32_t struct_size;
    uint32_t magic;
    uint32_t abi_version;
    uint32_t device_kind;
    uint32_t state_version;
    uint32_t ui_version;
    uint16_t package_major;
    uint16_t package_minor;
    uint16_t package_patch;
    uint16_t reserved0;
    uint64_t capabilities;
    uint32_t reported_latency_frames;
    uint32_t tail_frames;
    uint32_t parameter_count;
    uint32_t max_state_bytes;
    uint32_t type_id_len;
    uint32_t reserved1;
    uint8_t type_id[HEX_PACKAGE_ABI_MAX_TYPE_ID_BYTES];
} HexAbiDescriptor;

typedef struct HexAbiParameterDescriptor {
    uint32_t struct_size;
    uint32_t id;
    float min;
    float max;
    float default_value;
    uint32_t flags;
    uint32_t name_len;
    uint32_t reserved;
    uint8_t name[HEX_PACKAGE_ABI_MAX_PARAM_NAME_BYTES];
} HexAbiParameterDescriptor;

typedef struct HexAbiParamEvent {
    uint32_t sample_offset;
    uint32_t parameter_id;
    float value;
    uint32_t reserved;
} HexAbiParamEvent;

typedef struct HexAbiNoteEvent {
    uint32_t sample_offset;
    uint32_t kind;
    uint8_t note;
    uint8_t channel;
    uint16_t reserved0;
    float velocity;
    float detune_cents;
    uint32_t reserved1;
} HexAbiNoteEvent;

typedef struct HexAbiProcessContext {
    uint32_t struct_size;
    uint32_t abi_version;
    uint32_t sample_rate;
    uint32_t frames;
    uint32_t param_event_count;
    uint32_t note_event_count;
    uint32_t flags;
    uint32_t reserved;
} HexAbiProcessContext;

typedef struct HexAbiVtableV1 {
    uint32_t struct_size;
    uint32_t abi_version;
    int32_t (*describe)(HexAbiDescriptor* out_descriptor);
    int32_t (*parameter_descriptor)(uint32_t index, HexAbiParameterDescriptor* out_param);
    int32_t (*create)(uint32_t sample_rate, void** out_instance);
    void (*destroy)(void* instance);
    int32_t (*set_state)(void* instance, const uint8_t* bytes, uint32_t len);
    int32_t (*get_state)(const void* instance, uint8_t* out_bytes, uint32_t capacity, uint32_t* out_len);
    int32_t (*process)(
        void* instance,
        const HexAbiProcessContext* ctx,
        const float* in_interleaved_stereo,
        float* out_interleaved_stereo,
        const HexAbiParamEvent* param_events,
        const HexAbiNoteEvent* note_events
    );
} HexAbiVtableV1;

/* Single exported symbol negotiated by the host loader. */
typedef int32_t (*HexDeviceAbiEntryFn)(uint32_t requested_abi_version, HexAbiVtableV1* out_vtable);

#define HEX_DEVICE_ABI_ENTRY_SYMBOL "hex_device_abi_v1_entry"

#ifdef __cplusplus
}
#endif

#endif
