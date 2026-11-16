#ifndef GRKIT_LIBRARY_CARD_IDENTITY_H
#define GRKIT_LIBRARY_CARD_IDENTITY_H
/* Read-only card identity, byte-for-byte compatible with serve::identity.
   cid is the kernel-normalized 128-bit SD CID (not an HTTP-supplied ID).
   Firmware acquisition and its controller-specific CRC-byte normalization
   belong in the target adapter. No filesystem writes or allocation occur. */
#include "library_sha256.h"
static int library_card_equal(const u8 *a,const u8 *b,u32 count)
{ for(u32 i=0;i<count;++i) if(a[i]!=b[i]) return 0; return 1; }
static u32 library_card_le32(const u8 *p)
{ return (u32)p[0]|((u32)p[1]<<8)|((u32)p[2]<<16)|((u32)p[3]<<24); }
static const u8 *library_card_serial(const u8 sector[512],int *exfat)
{
    if(!sector || !exfat || sector[510]!=0x55 || sector[511]!=0xaa) return 0;
    *exfat=library_card_equal(sector+3,(const u8 *)"EXFAT   ",8);
    const u8 *serial=0;
    if(*exfat) { if(sector[105]!=1) return 0; serial=sector+100; }
    else if(library_card_equal(sector+82,(const u8 *)"FAT32   ",8) && sector[66]==0x29) serial=sector+67;
    else if((library_card_equal(sector+54,(const u8 *)"FAT16   ",8) ||
             library_card_equal(sector+54,(const u8 *)"FAT12   ",8)) && sector[38]==0x29) serial=sector+39;
    if(!serial || !(serial[0]|serial[1]|serial[2]|serial[3])) return 0;
    return serial;
}
static int library_card_identity(const u8 cid[16],const u8 sector[512],char out[70])
{
    if(!cid || !out) return 0;
    int exfat=0; const u8 *serial=library_card_serial(sector,&exfat);
    if(!serial) return 0;
    static const char hex[]="0123456789abcdef";
    u8 input[41],digest[32]; u32 length=32;
    for(u32 i=0;i<16;++i) { u8 byte=i==15?0:cid[i]; input[2*i]=(u8)hex[byte>>4]; input[2*i+1]=(u8)hex[byte&15]; }
    const char *kind=exfat?"exfat":"fat";
    for(u32 i=0;kind[i];++i) input[length++]=(u8)kind[i];
    for(u32 i=0;i<4;++i) input[length++]=serial[i];
    if(!library_sha256(input,length,digest)) return 0;
    for(u32 i=0;i<5;++i) out[i]="card-"[i];
    for(u32 i=0;i<32;++i) { out[5+2*i]=hex[digest[i]>>4]; out[6+2*i]=hex[digest[i]&15]; }
    out[69]=0; return 1;
}
/* Accept a superfloppy or exactly one bounded primary partition. Multiple
   volumes, extended partitions and GPT require an established mount mapping;
   guessing the first FAT volume could identify a different mounted card. */
static int library_card_volume_lba(const u8 sector[512],u32 capacity,u32 *lba)
{
    if(!sector || !lba || !capacity) return 0;
    int exfat; if(library_card_serial(sector,&exfat)) { *lba=0; return 1; }
    if(sector[510]!=0x55 || sector[511]!=0xaa) return 0;
    u32 found=0,start=0;
    for(u32 i=0;i<4;++i) {
        const u8 *entry=sector+446+16*i;
        u32 type=entry[4],at=library_card_le32(entry+8),count=library_card_le32(entry+12);
        if(!type && !at && !count) continue;
        if((entry[0]!=0 && entry[0]!=0x80) || !at || !count || at>=capacity || count>capacity-at) return 0;
        if(type!=1 && type!=4 && type!=6 && type!=7 && type!=0x0b && type!=0x0c && type!=0x0e) return 0;
        if(++found!=1) return 0;
        start=at;
    }
    if(found!=1) return 0;
    *lba=start; return 1;
}
struct library_card_reader {
    void *context;
    u32 capacity;
    int (*cid)(void *,u8 [16]);
    int (*sector)(void *,u32,u8 [512]);
};
/* The target must ensure these callbacks run on the resident task. Read both
   identity sources again before publication to detect replacement/reformat. */
struct library_card_scratch {
    u8 first[16],last[16];
    u8 sector[512] __attribute__((aligned(64)));
    u8 repeat[512] __attribute__((aligned(64)));
    u8 mbr[512];
};
static int library_card_read_evidence(const struct library_card_reader *reader,u8 out[532],struct library_card_scratch *scratch)
{
    if(!reader || !reader->cid || !reader->sector || !out || !scratch) return 0;
    u8 *first=scratch->first,*last=scratch->last;
    u8 *sector=scratch->sector,*repeat=scratch->repeat,*mbr=scratch->mbr;
    u32 lba;
    if(!reader->cid(reader->context,first) || !reader->sector(reader->context,0,sector) ||
       !library_card_volume_lba(sector,reader->capacity,&lba)) return 0;
    for(u32 i=0;i<512;++i) mbr[i]=sector[i];
    if(lba && !reader->sector(reader->context,lba,sector)) return 0;
    if(!reader->cid(reader->context,last) || !library_card_equal(first,last,16) ||
       !reader->sector(reader->context,lba,repeat) || !library_card_equal(sector,repeat,512)) return 0;
    if(lba && (!reader->sector(reader->context,0,repeat) || !library_card_equal(mbr,repeat,512))) return 0;
    int exfat;
    if(!library_card_serial(sector,&exfat)) return 0;
    for(u32 i=0;i<4;++i) out[i]=(u8)"GRC1"[i];
    for(u32 i=0;i<16;++i) out[4+i]=first[i];
    for(u32 i=0;i<512;++i) out[20+i]=sector[i];
    return 1;
}
static int library_card_read_identity(const struct library_card_reader *reader,char out[70],u8 evidence[532],struct library_card_scratch *scratch)
{
    return library_card_read_evidence(reader,evidence,scratch) && library_card_identity(evidence+4,evidence+20,out);
}
#endif
