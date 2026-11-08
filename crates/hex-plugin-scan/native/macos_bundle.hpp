// Bounded module-path/byte inspection shared by native helper roles.
// Reading metadata/hashes never loads module code; identities are not authenticity.
#pragma once
#import <Foundation/Foundation.h>
#include <CommonCrypto/CommonDigest.h>
#include <array>
#include <cstdio>
#include <cstdlib>
#include <fstream>
#include <memory>
#include <optional>
#include <stdexcept>
#include <string>
namespace hex_bundle {
constexpr size_t bundle_path_limit=4096;
static std::string utf8(NSString* value,size_t maximum) {
    if(![value isKindOfClass:[NSString class]]) throw std::runtime_error("bundle string missing");
    const char* bytes=[value UTF8String];
    auto size=[value lengthOfBytesUsingEncoding:NSUTF8StringEncoding];
    if(!bytes || size>maximum) throw std::runtime_error("bundle text bound or encoding refused");
    std::string result(bytes,size);
    if(result.find('\0')!=std::string::npos) throw std::runtime_error("bundle text null refused");
    return result;
}
static NSString* string(const std::string& value) {
    NSString* result=[[NSString alloc] initWithBytes:value.data() length:value.size() encoding:NSUTF8StringEncoding];
    if(!result) throw std::runtime_error("bundle path encoding refused");
    return [result autorelease];
}
static std::string canonical(const std::string& path) {
    std::unique_ptr<char,decltype(&std::free)> value(::realpath(path.c_str(),nullptr),&std::free);
    if(!value) throw std::runtime_error("bundle filesystem resolution failed");
    std::string result(value.get());
    if(result.size()>bundle_path_limit) throw std::runtime_error("canonical bundle path bound");
    return result;
}
static NSString* member(NSString* root,NSString* relative) {
    NSString* current=root;
    auto components=[relative pathComponents];
    if(![components count] || [components count]>32) throw std::runtime_error("bundle member depth refused");
    for(NSString* part in components) {
        if([part isEqual:@".."] || [part isEqual:@"."] || [part isEqual:@"/"]) throw std::runtime_error("bundle member escape refused");
        current=[current stringByAppendingPathComponent:part];
        NSDictionary* info=[[NSFileManager defaultManager] attributesOfItemAtPath:current error:nullptr];
        if(!info || [[info fileType] isEqual:NSFileTypeSymbolicLink]) throw std::runtime_error("linked/missing bundle member");
    }
    NSDictionary* info=[[NSFileManager defaultManager] attributesOfItemAtPath:current error:nullptr];
    if(![[info fileType] isEqual:NSFileTypeRegular]) throw std::runtime_error("nonregular bundle input");
    return current;
}
static std::string digest(NSString* path,std::optional<uint64_t> maximum=std::nullopt) {
    NSDictionary* info=[[NSFileManager defaultManager] attributesOfItemAtPath:path error:nullptr];
    if(!info || ![[info fileType] isEqual:NSFileTypeRegular]) throw std::runtime_error("nonregular scan input");
    uint64_t remaining=[info fileSize];
    if(maximum && remaining>*maximum) throw std::runtime_error("scan file bound refused");
    std::ifstream input([path fileSystemRepresentation],std::ios::binary);
    if(!input) throw std::runtime_error("scan file open failed");
    CC_SHA256_CTX context{}; CC_SHA256_Init(&context);
    std::array<char,65536> block{};
    // Bound reads by the observed length, with constant scratch regardless of module size.
    while(remaining) {
        auto count=remaining<block.size()?static_cast<size_t>(remaining):block.size();
        if(!input.read(block.data(),count)) throw std::runtime_error("scan file read failed or truncated");
        CC_SHA256_Update(&context,block.data(),static_cast<CC_LONG>(count));
        remaining-=count;
    }
    if(input.peek()!=std::ifstream::traits_type::eof()) throw std::runtime_error("scan file grew during validation");
    if(!input.eof() || input.bad()) throw std::runtime_error("scan file end read failed");
    std::array<unsigned char,CC_SHA256_DIGEST_LENGTH> hash{}; CC_SHA256_Final(hash.data(),&context);
    char result[65]={};
    for(size_t i=0;i<hash.size();++i) std::snprintf(result+2*i,3,"%02x",hash[i]);
    return result;
}
static std::string executable(CFBundleRef bundle,NSString* root) {
    CFURLRef selected=CFBundleCopyExecutableURL(bundle);
    CFURLRef absolute=selected?CFURLCopyAbsoluteURL(selected):nullptr;
    CFStringRef path=absolute?CFURLCopyFileSystemPath(absolute,kCFURLPOSIXPathStyle):nullptr;
    std::string relative;
    try {
        NSString* prefix=[root stringByAppendingString:@"/"];
        NSString* value=path?(__bridge NSString*)path:nil;
        if(!value || ![value hasPrefix:prefix]) throw std::runtime_error("executable outside selected bundle");
        NSString* result=[value substringFromIndex:[prefix length]];
        if(![result hasPrefix:@"Contents/MacOS/"]) throw std::runtime_error("native executable location refused");
        member(root,result);
        relative=utf8(result,bundle_path_limit);
    } catch(...) {
        if(path) CFRelease(path); if(absolute) CFRelease(absolute); if(selected) CFRelease(selected);
        throw;
    }
    if(path) CFRelease(path); if(absolute) CFRelease(absolute); if(selected) CFRelease(selected);
    return relative;
}

} // namespace hex_bundle
