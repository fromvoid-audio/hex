// Bounded shared-memory SPSC ring buffer for per-quantum VST3 PCM transport.
// Must match third_party/hex-shm-ring/src/raw.rs byte-for-byte.
#pragma once
#include <atomic>
#include <cerrno>
#include <chrono>
#include <cstdint>
#include <cstring>
#include <poll.h>
#include <sys/mman.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <unistd.h>

namespace hex_instance_shm {
constexpr uint8_t DOORBELL_BYTE = 0x01;
constexpr uint8_t SCM_HANDSHAKE_BYTE = 'S';
constexpr uint32_t VERSION = 1;
constexpr size_t CACHE_LINE_BYTES = 128;
constexpr size_t SLOT_COUNT = 4;
constexpr size_t SLOT_CAPACITY = 16384;
constexpr size_t HEADER_BYTES = 4096;
constexpr size_t REQ_SLOTS_OFFSET = HEADER_BYTES;
constexpr size_t REP_SLOTS_OFFSET = HEADER_BYTES + SLOT_COUNT * SLOT_CAPACITY;
constexpr size_t REGION_BYTES = HEADER_BYTES + 2 * SLOT_COUNT * SLOT_CAPACITY;
constexpr std::chrono::milliseconds HANDSHAKE_TIMEOUT{5000};

static_assert(REGION_BYTES == 135168, "shared-memory layout size mismatch");
static_assert(std::atomic<uint64_t>::is_always_lock_free, "atomic u64 must be lock-free");
static_assert(std::atomic<uint32_t>::is_always_lock_free, "atomic u32 must be lock-free");

// Header field byte offsets (aligned to 128-byte Apple Silicon cache lines).
// Line 0 (0..128): immutable header validated on attach.
constexpr size_t OFF_MAGIC = 0;
constexpr size_t OFF_VERSION = 8;
constexpr size_t OFF_SLOT_COUNT = 12;
constexpr size_t OFF_SLOT_CAPACITY = 16;
constexpr size_t OFF_RESERVED = 20;

// Line 1 (128..256): Host-written request producer line.
constexpr size_t OFF_REQ_WRITE_SEQ = 128;
constexpr size_t OFF_REQ_SLOT_LEN = 136;

// Line 2 (256..384): Worker-written request consumer line.
constexpr size_t OFF_REQ_READ_SEQ = 256;

// Line 3 (384..512): Worker-written reply producer line.
constexpr size_t OFF_REP_WRITE_SEQ = 384;
constexpr size_t OFF_REP_SLOT_LEN = 392;

// Line 4 (512..640): Host-written reply consumer line.
constexpr size_t OFF_REP_READ_SEQ = 512;

// Line 5 (640..768): Shared fault indicator line.
constexpr size_t OFF_FAULT_FLAG = 640;

static_assert(OFF_REQ_WRITE_SEQ % CACHE_LINE_BYTES == 0, "req write seq must be 128 B aligned");
static_assert(OFF_REQ_READ_SEQ % CACHE_LINE_BYTES == 0, "req read seq must be 128 B aligned");
static_assert(OFF_REP_WRITE_SEQ % CACHE_LINE_BYTES == 0, "rep write seq must be 128 B aligned");
static_assert(OFF_REP_READ_SEQ % CACHE_LINE_BYTES == 0, "rep read seq must be 128 B aligned");
static_assert(OFF_FAULT_FLAG % CACHE_LINE_BYTES == 0, "fault flag must be 128 B aligned");
static_assert(OFF_REQ_WRITE_SEQ / CACHE_LINE_BYTES != OFF_REQ_READ_SEQ / CACHE_LINE_BYTES,
              "req producer and consumer counters must occupy distinct 128 B cache lines");
static_assert(OFF_REP_WRITE_SEQ / CACHE_LINE_BYTES != OFF_REP_READ_SEQ / CACHE_LINE_BYTES,
              "rep producer and consumer counters must occupy distinct 128 B cache lines");
static_assert(OFF_REQ_SLOT_LEN + SLOT_COUNT * sizeof(uint32_t) <= OFF_REQ_READ_SEQ,
              "req slot lengths must remain on the req producer cache line");
static_assert(OFF_REP_SLOT_LEN + SLOT_COUNT * sizeof(uint32_t) <= OFF_REP_READ_SEQ,
              "rep slot lengths must remain on the rep producer cache line");
static_assert(OFF_FAULT_FLAG + sizeof(uint32_t) <= HEADER_BYTES,
              "shared-memory header fields must fit in HEADER_BYTES");

class ShmRing {
    uint8_t* base_ = nullptr;

    // Access mapped scalar words with atomic builtins rather than fabricating
    // `std::atomic<T>` object lifetimes via `reinterpret_cast` in C++17.
    uint32_t load_u32(size_t offset, int order) const {
        return __atomic_load_n(reinterpret_cast<const uint32_t*>(base_ + offset), order);
    }
    void store_u32(size_t offset, uint32_t value, int order) const {
        __atomic_store_n(reinterpret_cast<uint32_t*>(base_ + offset), value, order);
    }
    uint64_t load_u64(size_t offset, int order) const {
        return __atomic_load_n(reinterpret_cast<const uint64_t*>(base_ + offset), order);
    }
    void store_u64(size_t offset, uint64_t value, int order) const {
        __atomic_store_n(reinterpret_cast<uint64_t*>(base_ + offset), value, order);
    }

    bool validate_header() const {
        return std::memcmp(base_ + OFF_MAGIC, "HXSHM001", 8) == 0 &&
               load_u32(OFF_VERSION, __ATOMIC_ACQUIRE) == VERSION &&
               load_u32(OFF_SLOT_COUNT, __ATOMIC_ACQUIRE) == SLOT_COUNT &&
               load_u32(OFF_SLOT_CAPACITY, __ATOMIC_ACQUIRE) == SLOT_CAPACITY &&
               load_u32(OFF_RESERVED, __ATOMIC_ACQUIRE) == 0;
    }

public:
    ShmRing() = default;
    ShmRing(const ShmRing&) = delete;
    ShmRing& operator=(const ShmRing&) = delete;
    ~ShmRing() { close(); }

    bool active() const { return base_ != nullptr; }

    void close() {
        if (base_) {
            ::munmap(base_, REGION_BYTES);
            base_ = nullptr;
        }
    }

    // Receive the host-allocated anonymous POSIX shared-memory descriptor via
    // SCM_RIGHTS, validate its size/header, mmap it, and close the fd immediately.
    bool recv_and_map(int socket_fd) {
        close();
        uint8_t dummy = 0;
        iovec iov{&dummy, sizeof(dummy)};
        alignas(cmsghdr) uint8_t cmsg_buf[CMSG_SPACE(sizeof(int))]{};
        msghdr msg{};
        msg.msg_iov = &iov;
        msg.msg_iovlen = 1;
        msg.msg_control = cmsg_buf;
        msg.msg_controllen = sizeof(cmsg_buf);

        const auto deadline = std::chrono::steady_clock::now() + HANDSHAKE_TIMEOUT;
        ssize_t received = -1;
        while (true) {
            msg.msg_controllen = sizeof(cmsg_buf);
            msg.msg_flags = 0;
            received = ::recvmsg(socket_fd, &msg, 0);
            if (received < 0 && errno == EINTR) continue;
            if (received < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)) {
                const auto now = std::chrono::steady_clock::now();
                if (now >= deadline) return false;
                const auto remaining_ms =
                    std::chrono::duration_cast<std::chrono::milliseconds>(deadline - now).count();
                const int wait_ms = remaining_ms <= 0 ? 1 : (remaining_ms > 10 ? 10 : static_cast<int>(remaining_ms));
                pollfd pfd{socket_fd, POLLIN, 0};
                const int ready = ::poll(&pfd, 1, wait_ms);
                if (ready < 0 && errno != EINTR) return false;
                if (ready > 0 && (pfd.revents & (POLLERR | POLLNVAL | POLLHUP))) return false;
                continue;
            }
            break;
        }
        if (received != 1) return false;

        int shm_fd = -1;
        bool valid_cmsg = false;
        for (cmsghdr* cmsg = CMSG_FIRSTHDR(&msg); cmsg != nullptr; cmsg = CMSG_NXTHDR(&msg, cmsg)) {
            if (cmsg->cmsg_level == SOL_SOCKET && cmsg->cmsg_type == SCM_RIGHTS) {
                const auto* cmsg_base = reinterpret_cast<const uint8_t*>(cmsg);
                const auto* data = CMSG_DATA(cmsg);
                const size_t hdr_len = static_cast<size_t>(data - cmsg_base);
                if (static_cast<size_t>(cmsg->cmsg_len) < hdr_len) {
                    continue;
                }
                const size_t data_len = static_cast<size_t>(cmsg->cmsg_len) - hdr_len;
                const size_t fd_count = data_len / sizeof(int);
                for (size_t i = 0; i < fd_count; ++i) {
                    int candidate = -1;
                    std::memcpy(&candidate, data + i * sizeof(int), sizeof(int));
                    if (candidate >= 0) {
                        if (shm_fd < 0 && fd_count == 1) {
                            shm_fd = candidate;
                            valid_cmsg = true;
                        } else {
                            ::close(candidate);
                            valid_cmsg = false;
                        }
                    }
                }
            }
        }
        if ((msg.msg_flags & MSG_CTRUNC) || dummy != SCM_HANDSHAKE_BYTE || !valid_cmsg || shm_fd < 0) {
            if (shm_fd >= 0) ::close(shm_fd);
            return false;
        }

        struct stat st{};
        if (::fstat(shm_fd, &st) != 0 || st.st_size < static_cast<off_t>(REGION_BYTES)) {
            ::close(shm_fd);
            return false;
        }
        void* mapped = ::mmap(nullptr, REGION_BYTES, PROT_READ | PROT_WRITE, MAP_SHARED, shm_fd, 0);
        ::close(shm_fd);
        if (mapped == MAP_FAILED || !mapped) return false;

        base_ = static_cast<uint8_t*>(mapped);
        if (!validate_header()) {
            close();
            return false;
        }
        return true;
    }

    bool read_request(uint8_t* dst, size_t dst_cap, size_t& out_len) const {
        if (!base_) return false;
        const uint64_t r = load_u64(OFF_REQ_READ_SEQ, __ATOMIC_RELAXED);
        const uint64_t w = load_u64(OFF_REQ_WRITE_SEQ, __ATOMIC_ACQUIRE);
        if (w <= r || (w - r) > SLOT_COUNT) return false;
        const size_t slot = static_cast<size_t>(r % SLOT_COUNT);
        const uint32_t len = load_u32(OFF_REQ_SLOT_LEN + slot * 4, __ATOMIC_RELAXED);
        if (len == 0 || len > SLOT_CAPACITY || len > dst_cap) return false;
        std::memcpy(dst, base_ + REQ_SLOTS_OFFSET + slot * SLOT_CAPACITY, len);
        store_u64(OFF_REQ_READ_SEQ, r + 1, __ATOMIC_RELEASE);
        out_len = len;
        return true;
    }

    bool write_reply(const uint8_t* src, size_t len) const {
        if (!base_ || len == 0 || len > SLOT_CAPACITY) return false;
        const uint64_t w = load_u64(OFF_REP_WRITE_SEQ, __ATOMIC_RELAXED);
        const uint64_t r = load_u64(OFF_REP_READ_SEQ, __ATOMIC_ACQUIRE);
        if (w < r || (w - r) >= SLOT_COUNT) return false;
        const size_t slot = static_cast<size_t>(w % SLOT_COUNT);
        std::memcpy(base_ + REP_SLOTS_OFFSET + slot * SLOT_CAPACITY, src, len);
        store_u32(OFF_REP_SLOT_LEN + slot * 4, static_cast<uint32_t>(len), __ATOMIC_RELAXED);
        store_u64(OFF_REP_WRITE_SEQ, w + 1, __ATOMIC_RELEASE);
        return true;
    }

    bool is_faulted() const {
        return !base_ || load_u32(OFF_FAULT_FLAG, __ATOMIC_ACQUIRE) != 0;
    }
};
} // namespace hex_instance_shm
