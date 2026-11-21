// Original signed observation profile only; never included by normal helpers.
#pragma once
#include <charconv>

// One processing-lane record, read only after join. No worker allocation,
// publication, file access or SDK/wire change. The interval brackets the SDK
// slice, not the exact time at which an individual output sample was generated.
class OriginalSliceClockObservation {
    std::array<uint64_t,6> target_{};
    // root, instance, graph, epoch, request sequence, request first frame,
    // slice, slice offset, slice width, Mach-absolute start/end ticks,
    // timebase numerator/denominator, helper PID, SDK success, reply sent,
    // post-join control publication. No hardware/presentation timestamp.
    std::array<uint64_t,17> record_{};
    std::array<char,4097> path_{};
    bool enabled_=false,captured_=false;
public:
    OriginalSliceClockObservation() {
        const auto* variant=std::getenv("HEX_ORIGINAL_INSTANCE_VARIANT");
        const auto* target=std::getenv("HEX_ORIGINAL_SLICE_CLOCK_TARGET");
        const auto* path=std::getenv("HEX_ORIGINAL_SLICE_CLOCK_PATH");
        if(!variant || std::strcmp(variant,"127") || !target || !path ||
            !*path || std::strlen(path)>4096 || std::strlen(target)>128) return;
        const auto* end=target+std::strlen(target);
        for(size_t n=0;n<target_.size();++n) {
            const auto value=std::from_chars(target,end,target_[n]);
            if(value.ec!=std::errc{} || (n<4 && !target_[n])) return;
            target=value.ptr;
            if(n+1<target_.size()) { if(target==end || *target++!=',') return; }
            else if(target!=end || target_[n]>7) return;
        }
        mach_timebase_info_data_t timebase{};
        if(mach_timebase_info(&timebase)!=KERN_SUCCESS || !timebase.numer || !timebase.denom) return;
        record_[11]=timebase.numer;record_[12]=timebase.denom;
        record_[13]=static_cast<uint64_t>(::getpid());
        std::memcpy(path_.data(),path,std::strlen(path)+1);enabled_=true;
    }
    bool begin(const hex_instance_wire::Config& config,uint64_t sequence,uint64_t frame,
        size_t slice,size_t offset,size_t width) {
        if(!enabled_ || captured_ || sequence!=target_[4] || slice!=target_[5]) return false;
        for(size_t n=0;n<4;++n) if(config.binding[n]!=target_[n]) return false;
        for(size_t n=0;n<4;++n) record_[n]=config.binding[n];
        record_[4]=sequence;record_[5]=frame;record_[6]=slice;
        record_[7]=offset;record_[8]=width;
        record_[9]=mach_absolute_time();return true;
    }
    void end(bool matched,bool processed) {
        if(!matched) return;
        record_[10]=mach_absolute_time();record_[14]=processed;captured_=true;
    }
    void replied(uint64_t sequence) {
        if(captured_ && sequence==record_[4]) record_[15]=1;
    }
    void publish_after_join() const {
        if(!captured_ || !record_[14] || !record_[15] || !pthread_main_np() ||
            !record_[9] || record_[10]<record_[9]) return;
        // HXMC0001 + 17 little-endian u64 fields, in record_ order above.
        // Final field witnesses control publication after the worker join.
        std::array<uint8_t,144> bytes{};
        std::memcpy(bytes.data(),"HXMC0001",8);
        for(size_t n=0;n<record_.size();++n) for(size_t byte=0;byte<8;++byte)
            bytes[8+n*8+byte]=static_cast<uint8_t>((n==16?1:record_[n])>>(byte*8));
        const int file=::open(path_.data(),O_WRONLY|O_CREAT|O_EXCL|O_NOFOLLOW,0600);
        if(file<0) return;
        const bool written=hex_instance_wire::write_exact(file,bytes.data(),bytes.size());
        const bool closed=::close(file)==0;
        if(!written || !closed) ::unlink(path_.data());
    }
};
