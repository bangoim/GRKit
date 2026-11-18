#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
typedef uint8_t u8;
typedef uint32_t u32;
#include "library_snapshot_stream.h"
#include "grf.h"
static u8 manifest_bytes[LIBRARY_STORE_MANIFEST_MAX],content[GRKIT_GRF_MAX_SIZE+1],frame[GRKIT_CONTROL_PAYLOAD_BYTES];
static struct library_manifest manifest;
static struct library_snapshot_stream stream;
static int valid=1,load_fault,live_calls,fail_on_live,loads;
static int validate_grf(const u8 *bytes,u32 length) { return grkit_grf_validate(bytes,length); }
static int live(void *context) { (void)context; ++live_calls; return valid && (!fail_on_live || live_calls!=fail_on_live); }
static int load(void *context,u32 index,const u8 **bytes,u32 *length)
{
    char path[1024]; assert(snprintf(path,sizeof(path),"%s/%u.grf",(const char *)context,index)<(int)sizeof(path));
    ++loads;
    FILE *file=fopen(path,"rb"); if(!file) return 0;
    *length=(u32)fread(content,1,sizeof(content),file); int failed=ferror(file); fclose(file);
    struct library_manifest_entry entry; assert(library_manifest_get(&manifest,index,&entry));
    if(failed || !library_content_matches(&entry,content,*length,validate_grf)) return 0;
    if(load_fault==1) return 0;
    if(load_fault==2) { memcpy(frame,content,*length); *bytes=frame; return 1; }
    if(load_fault==3) content[0]^=1;
    *bytes=content; return 1;
}
static void request(u8 bytes[128]) { memcpy(bytes,stream.header,128); library_store_put(bytes+76,stream.next); }
static void output(u32 length) { u8 word[4]; library_store_put(word,length); assert(fwrite(word,1,4,stdout)==4); assert(fwrite(frame,1,length,stdout)==length); }
int main(int argc,char **argv)
{
    assert(argc==3); FILE *file=fopen(argv[1],"rb"); assert(file);
    u32 size=(u32)fread(manifest_bytes,1,sizeof(manifest_bytes),file); assert(!ferror(file)); fclose(file);
    assert(library_manifest_open(manifest_bytes,size,&manifest));
    u32 loaded[40],count=0; for(u32 i=0;i<manifest.entries;++i) if(library_store_word(library_manifest_record(&manifest,i)+84)&1) loaded[count++]=i;
    struct library_dependencies deps={0}; struct library_snapshot_metadata meta={0};
    meta.manifest=&manifest; meta.card_id="card-test"; meta.epoch[0]=1; meta.epoch[1]=2; meta.epoch[2]=3; meta.epoch[3]=4;
    meta.generation=7; meta.loaded=loaded; meta.loaded_count=count; meta.selected=256;
    meta.dependencies=&deps; meta.dependencies_complete=1; meta.supported_features=0xffffffffU;
    struct library_snapshot_io io={argv[2],load,live}; u8 token[16]={1},req[128]; u32 length;
    /* Invalid destinations cannot acquire a stream or retain a stale length. */
    length=99; assert(!library_snapshot_begin(&stream,&meta,token,&io,1,0,sizeof(frame),&length));
    assert(!stream.active && !length);
    length=99; assert(!library_snapshot_begin(&stream,&meta,token,&io,1,frame,128,&length));
    assert(!stream.active && !length);
    assert(!library_snapshot_begin(&stream,&meta,token,&io,1,(u8 *)&stream,sizeof(stream),&length));
    assert(!library_snapshot_begin(&stream,&meta,token,&io,1,manifest_bytes,sizeof(manifest_bytes),&length));
    assert(!stream.active && !length);
    /* Even failure after bytes were copied must publish no successful length. */
    live_calls=0; fail_on_live=3;
    assert(!library_snapshot_begin(&stream,&meta,token,&io,1,frame,sizeof(frame),&length));
    assert(!stream.active && !length); fail_on_live=0;
    if(manifest.listed) {
        /* Without a description of these bytes, BEGIN reads and checks each content. */
        for(load_fault=1;load_fault<=3;++load_fault) {
            stream.described=0;
            length=99; assert(!library_snapshot_begin(&stream,&meta,token,&io,1,frame,sizeof(frame),&length));
            assert(!stream.active && !length);
        }
        load_fault=0;
        /* A later content callback failure invalidates the pin, even when
           preceding chunks contained only metadata. */
        assert(library_snapshot_begin(&stream,&meta,token,&io,1,frame,129,&length));
        load_fault=1;
        u32 attempts=0;
        do {
            request(req); ++attempts;
            if(!library_snapshot_read(&stream,&meta,&io,2,req,sizeof(req),frame,sizeof(frame),&length)) break;
            assert(attempts<128);
        } while(stream.active);
        assert(!stream.active && !length); load_fault=0;
    }
    if(count) {
        meta.loaded_count=0;
        assert(!library_snapshot_begin(&stream,&meta,token,&io,1,frame,sizeof(frame),&length));
        assert(!stream.active && !length); meta.loaded_count=count;
        for(u32 i=0;i<manifest.entries;++i) if(library_store_word(library_manifest_record(&manifest,i)+84)&2U) {
            meta.selected=i;
            assert(!library_snapshot_begin(&stream,&meta,token,&io,1,frame,sizeof(frame),&length));
            assert(!stream.active && !length); meta.selected=256; break;
        }
    }
    assert(library_snapshot_begin(&stream,&meta,token,&io,1,frame,sizeof(frame),&length));
    assert(!library_snapshot_begin(&stream,&meta,token,&io,2,frame,sizeof(frame),&length));
    request(req); req[4]^=1;
    assert(!library_snapshot_read(&stream,&meta,&io,2,req,sizeof(req),frame,sizeof(frame),&length));
    request(req); library_store_put(req+76,stream.next+1);
    assert(!library_snapshot_read(&stream,&meta,&io,2,req,sizeof(req),frame,sizeof(frame),&length));
    library_snapshot_expire(&stream,LIBRARY_STREAM_TTL_MS+2); assert(!stream.active);
    ++token[0]; assert(library_snapshot_begin(&stream,&meta,token,&io,3,frame,sizeof(frame),&length));
    valid=0; request(req); assert(!library_snapshot_end(&stream,&io,4,req,sizeof(req))); assert(!stream.active); valid=1;
    ++token[0]; assert(library_snapshot_begin(&stream,&meta,token,&io,5,frame,sizeof(frame),&length)); output(length);
    while(stream.next<library_store_word(stream.header+72)) {
        request(req);
        assert(library_snapshot_read(&stream,&meta,&io,6,req,sizeof(req),frame,sizeof(frame),&length)); output(length);
        assert(!library_snapshot_read(&stream,&meta,&io,6,req,sizeof(req),frame,sizeof(frame),&length));
    }
    request(req); assert(library_snapshot_end(&stream,&io,7,req,sizeof(req))); assert(!stream.active);
    assert(!library_snapshot_end(&stream,&io,7,req,sizeof(req)));
    /* A reader that holds the bytes of this digest releases the pin after
       BEGIN. The request must still name the stream and its next offset. */
    ++token[0]; assert(library_snapshot_begin(&stream,&meta,token,&io,8,frame,sizeof(frame),&length));
    request(req); library_store_put(req+76,stream.next+1);
    assert(!library_snapshot_end(&stream,&io,8,req,sizeof(req))); assert(stream.active);
    request(req); req[4]^=1;
    assert(!library_snapshot_end(&stream,&io,8,req,sizeof(req))); assert(stream.active);
    request(req); assert(library_snapshot_end(&stream,&io,8,req,sizeof(req))); assert(!stream.active);
    /* Equal metadata describes equal bytes: BEGIN reuses the digest and
       loads only the content of its first frame. */
    u8 digest[32]; memcpy(digest,stream.header+80,32); u32 total=library_store_word(stream.header+72);
    loads=0; ++token[0]; assert(library_snapshot_begin(&stream,&meta,token,&io,9,frame,sizeof(frame),&length));
    assert(!memcmp(stream.header+80,digest,32) && library_store_word(stream.header+72)==total);
    assert(loads<=1);
    request(req); assert(library_snapshot_end(&stream,&io,9,req,sizeof(req)));
    /* Other metadata reads and hashes every content again. */
    meta.generation=8; loads=0; ++token[0];
    assert(library_snapshot_begin(&stream,&meta,token,&io,10,frame,sizeof(frame),&length));
    assert(loads>=(int)stream.content_count && memcmp(stream.header+80,digest,32));
    while(stream.next<library_store_word(stream.header+72)) {
        request(req); assert(library_snapshot_read(&stream,&meta,&io,10,req,sizeof(req),frame,sizeof(frame),&length));
    }
    request(req); assert(library_snapshot_end(&stream,&io,10,req,sizeof(req)));
    return 0;
}
