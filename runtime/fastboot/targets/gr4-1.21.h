/* GR IV 1.21: the RTOS calls of the fast-boot loader. The file calls are those
   of the file-system task, which the RTOS serves for every task. */
#ifndef GRKIT_FASTBOOT_TARGET_H
#define GRKIT_FASTBOOT_TARGET_H

#define RT_PRINTF 0x53914fb4  /* printf to the serial console */
#define RT_LOG 0x53915138  /* the RTOS log: (0, 0, 0, 0, format, ...) */
#define RT_BF_OPEN 0x53abf500  /* BF_Fs_If_Open(name, mode) -> fNo, 0 on failure */
#define RT_BF_READ 0x53abf56c  /* BF_Fs_If_Read(fNo, address, count, &length) -> 0 */
#define RT_BF_CLOSE 0x53abf53c  /* BF_Fs_If_Close(fNo) */
#define RT_FS_STAT_SIZE 0x53ac0004  /* BF_Fs_If_Stat(name, out) -> 0; out[2] is the size */
#define RT_FS_OPEN 0x53ac3ccc  /* open(name, mode, 0) -> fNo >= 0 */
#define RT_FS_CLOSE 0x53ac3d80  /* close(fNo) */
#define RT_FS_READ 0x53ac3de8  /* read(fNo, address, count, &length) -> 0 */
#define RT_FS_WRITE 0x53ac3e5c  /* write(fNo, address, count, &length) -> 0 */
#define RT_FS_CREATE 0x53ac3ed0  /* create(name, 0) -> fNo >= 0 */
#define RT_FS_REMOVE 0x53ac3f7c  /* remove(name) -> 0 */
#define RT_FS_MKDIR 0x53ac4018  /* mkdir(name) -> 0 */
#define RT_FS_OPENDIR 0x53ac4150  /* opendir(name, &dNo) -> 0 */
#define RT_FS_READDIR 0x53ac41f4  /* readdir(dNo, entry) -> 0, 1 after the last entry */
#define RT_FS_CLOSEDIR 0x53ac4560  /* closedir(dNo) */
#define RT_FS_RENAME 0x53ac4644  /* rename(source, destination) -> 0 */

/* Read-only SD identity through the native media manager and SdDriver. */
#define RT_CARD_MANAGER 0x5537fbe4U
#define RT_CARD_ENTRY_SIZE 0x0000001cU
#define RT_CARD_VTABLE 0x54388ad8U
/* +0x2c is WRITE (CMD24); +0x30 reaches CMD18 with TransferMode.READ. */
#define RT_CARD_READ_SLOT 0x30U
#define RT_CARD_READ 0x53a75774U
#define RT_CARD_CID 0x550764e0U

#endif
