#include <ntddk.h>
#include <wdf.h>
#include <intrin.h>
#include "rapl_ioctl.h"

#ifndef RAPL_ENABLE_LIVE_WRITES
#define RAPL_ENABLE_LIVE_WRITES 0
#endif

#define NT_DEVICE_NAME L"\\Device\\RaplPkg"
#define DOS_DEVICE_NAME L"\\DosDevices\\RaplPkg"

DRIVER_INITIALIZE DriverEntry;
EVT_WDF_DRIVER_DEVICE_ADD RaplEvtDeviceAdd;
EVT_WDF_IO_QUEUE_IO_DEVICE_CONTROL RaplEvtIoDeviceControl;

static BOOLEAN RaplIsSupportedCpu(_Out_ ULONG* model, _Out_ ULONG* stepping) {
    int cpu[4] = {0};
    __cpuidex(cpu, 1, 0);
    ULONG family = ((ULONG)cpu[0] >> 8) & 0xf;
    ULONG ext_family = ((ULONG)cpu[0] >> 20) & 0xff;
    ULONG base_model = ((ULONG)cpu[0] >> 4) & 0xf;
    ULONG ext_model = ((ULONG)cpu[0] >> 16) & 0xf;
    if (family == 0xf) family += ext_family;
    if (family == 6 || family == 0xf) base_model |= ext_model << 4;
    *model = base_model;
    *stepping = (ULONG)cpu[0] & 0xf;
    // Raptor Lake mobile profile. Unknown models remain read/write blocked.
    return family == 6 && base_model == 0xb7;
}

static NTSTATUS RaplValidateRequest(const RAPL_REQUEST* request, ULONG expectedId) {
    if (request == NULL || request->Size < sizeof(RAPL_REQUEST) ||
        request->Version != RAPL_PROTOCOL_VERSION || request->Reserved != 0 ||
        request->RegisterId != expectedId) return STATUS_INVALID_PARAMETER;
    if (expectedId == RaplRegisterUnits && request->Value != 0) return STATUS_INVALID_PARAMETER;
    return STATUS_SUCCESS;
}

static NTSTATUS RaplReadMsr(ULONG id, _Out_ ULONGLONG* value) {
    if (!value) return STATUS_INVALID_PARAMETER;
    if (id != RaplRegisterUnits && id != RaplRegisterPackageLimit) return STATUS_INVALID_PARAMETER;
    __try {
        *value = __readmsr(id == RaplRegisterUnits ? RAPL_MSR_UNIT : RAPL_MSR_PACKAGE_LIMIT);
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        return GetExceptionCode();
    }
    return STATUS_SUCCESS;
}

static NTSTATUS RaplWriteMsr(const RAPL_REQUEST* req, _Out_ ULONGLONG* readback) {
#if !RAPL_ENABLE_LIVE_WRITES
    UNREFERENCED_PARAMETER(req); UNREFERENCED_PARAMETER(readback);
    return STATUS_NOT_SUPPORTED;
#else
    ULONGLONG current = 0;
    NTSTATUS status = RaplReadMsr(RaplRegisterPackageLimit, &current);
    if (!NT_SUCCESS(status) || current != req->ExpectedValue) return STATUS_RETRY;
    if ((current & RAPL_LOCK_BIT) != 0 || (req->Value & RAPL_LOCK_BIT) != 0) return STATUS_ACCESS_DENIED;
    if (((current ^ req->Value) & ~RAPL_POWER_FIELD_MASK) != 0) return STATUS_INVALID_PARAMETER;
    __try { __writemsr(RAPL_MSR_PACKAGE_LIMIT, req->Value); } __except (EXCEPTION_EXECUTE_HANDLER) { return GetExceptionCode(); }
    status = RaplReadMsr(RaplRegisterPackageLimit, readback);
    if (!NT_SUCCESS(status) || *readback != req->Value) return STATUS_DATA_ERROR;
    return STATUS_SUCCESS;
#endif
}

static NTSTATUS RaplBuildInfo(_Out_ RAPL_INFO* info) {
    ULONG model = 0, stepping = 0;
    if (!info) return STATUS_INVALID_PARAMETER;
    RtlZeroMemory(info, sizeof(*info));
    info->Size = sizeof(*info);
    info->Version = RAPL_PROTOCOL_VERSION;
    info->Vendor = 0x8086;
    info->PackageCount = 1;
    if (!RaplIsSupportedCpu(&model, &stepping)) {
        info->Model = model; info->Stepping = stepping;
        info->Flags = RAPL_FLAG_READ_ONLY;
        return STATUS_SUCCESS;
    }
    info->Model = model;
    info->Stepping = stepping;
    info->Flags = RAPL_FLAG_READ_ONLY;
    // MMIO verification is deliberately false until PCI BAR + 0x59A0 validation exists.
    return STATUS_SUCCESS;
}

static NTSTATUS RaplCompleteResponse(WDFREQUEST request, ULONG_PTR information,
                                     NTSTATUS status) {
    WdfRequestCompleteWithInformation(request, status, information);
    return status;
}

VOID RaplEvtIoDeviceControl(WDFQUEUE queue, WDFREQUEST request, size_t inputLength,
                            size_t outputLength, ULONG ioControlCode) {
    UNREFERENCED_PARAMETER(queue);
    NTSTATUS status = STATUS_INVALID_DEVICE_REQUEST;
    ULONG_PTR information = 0;
    PVOID input = NULL, output = NULL;

    if (ioControlCode == IOCTL_RAPL_GET_INFO) {
        if (outputLength < sizeof(RAPL_INFO)) goto done;
        status = WdfRequestRetrieveOutputBuffer(request, sizeof(RAPL_INFO), &output, NULL);
        if (!NT_SUCCESS(status)) goto done;
        status = RaplBuildInfo((RAPL_INFO*)output);
        if (NT_SUCCESS(status)) information = sizeof(RAPL_INFO);
        goto done;
    }
    if (inputLength < sizeof(RAPL_REQUEST) || outputLength < sizeof(RAPL_RESPONSE)) goto done;
    status = WdfRequestRetrieveInputBuffer(request, sizeof(RAPL_REQUEST), &input, NULL);
    if (!NT_SUCCESS(status)) goto done;
    status = WdfRequestRetrieveOutputBuffer(request, sizeof(RAPL_RESPONSE), &output, NULL);
    if (!NT_SUCCESS(status)) goto done;
    RAPL_REQUEST* req = (RAPL_REQUEST*)input;
    RAPL_RESPONSE* rsp = (RAPL_RESPONSE*)output;
    RtlZeroMemory(rsp, sizeof(*rsp));
    rsp->Size = sizeof(*rsp); rsp->Version = RAPL_PROTOCOL_VERSION;
    rsp->RegisterId = req->RegisterId;

    if (ioControlCode == IOCTL_RAPL_READ_MSR) {
        status = RaplValidateRequest(req, req->RegisterId);
        if (NT_SUCCESS(status)) status = RaplReadMsr(req->RegisterId, &rsp->Value);
        if (NT_SUCCESS(status)) information = sizeof(*rsp);
        goto done;
    }
    if (ioControlCode == IOCTL_RAPL_READ_MMIO) {
        status = RaplValidateRequest(req, RaplRegisterMmioPackageLimit);
        // No guessed physical address and no unverified MCHBAR mapping.
        if (NT_SUCCESS(status)) status = STATUS_NOT_SUPPORTED;
        goto done;
    }
    if (ioControlCode == IOCTL_RAPL_WRITE_MSR || ioControlCode == IOCTL_RAPL_WRITE_MMIO) {
if (ioControlCode == IOCTL_RAPL_WRITE_MSR) {
            status = RaplValidateRequest(req, RaplRegisterPackageLimit);
            if (NT_SUCCESS(status)) status = RaplWriteMsr(req, &rsp->Value);
            if (NT_SUCCESS(status)) information = sizeof(*rsp);
        } else {
            // MCHBAR discovery and 32-bit ordering are not yet validated for this platform.
            status = STATUS_NOT_SUPPORTED;
        }
        goto done;
    }

done:
    RaplCompleteResponse(request, information, status);
}

NTSTATUS RaplEvtDeviceAdd(WDFDRIVER driver, PWDFDEVICE_INIT deviceInit) {
    UNREFERENCED_PARAMETER(driver);
    NTSTATUS status;
    WDFDEVICE device;
    WDF_IO_QUEUE_CONFIG queueConfig;
    UNICODE_STRING dosName, deviceName, sddl;

    WdfDeviceInitSetDeviceType(deviceInit, FILE_DEVICE_UNKNOWN);
    RtlInitUnicodeString(&deviceName, NT_DEVICE_NAME);
    status = WdfDeviceInitAssignName(deviceInit, &deviceName);
    if (!NT_SUCCESS(status)) return status;
    RtlInitUnicodeString(&sddl, L"D:P(A;;GA;;;SY)(A;;GR;;;BA)");
    status = WdfDeviceInitAssignSDDLString(deviceInit, &sddl);
    if (!NT_SUCCESS(status)) return status;
    status = WdfDeviceCreate(&deviceInit, WDF_NO_OBJECT_ATTRIBUTES, &device);
    if (!NT_SUCCESS(status)) return status;
    RtlInitUnicodeString(&dosName, DOS_DEVICE_NAME);
    status = WdfDeviceCreateSymbolicLink(device, &dosName);
    if (!NT_SUCCESS(status)) return status;
    WDF_IO_QUEUE_CONFIG_INIT_DEFAULT_QUEUE(&queueConfig, WdfIoQueueDispatchSequential);
    queueConfig.EvtIoDeviceControl = RaplEvtIoDeviceControl;
    return WdfIoQueueCreate(device, &queueConfig, WDF_NO_OBJECT_ATTRIBUTES, WDF_NO_HANDLE);
}

NTSTATUS DriverEntry(PDRIVER_OBJECT driverObject, PUNICODE_STRING registryPath) {
    WDF_DRIVER_CONFIG config;
    WDF_DRIVER_CONFIG_INIT(&config, RaplEvtDeviceAdd);
    return WdfDriverCreate(driverObject, registryPath, WDF_NO_OBJECT_ATTRIBUTES, &config, WDF_NO_HANDLE);
}
