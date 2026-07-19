package com.example.chatapp.manager

import com.example.chatapp.model.DownloadProgress
import com.example.chatapp.model.DownloadState
import com.example.chatapp.repository.FakeDownloadRepository
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.test.runTest
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.BeforeEach
import org.junit.jupiter.api.Test

class DownloadManagerTest {

    private lateinit var downloadManager: DownloadManager
    private lateinit var repository: FakeDownloadRepository

    @BeforeEach
    fun setup() {
        repository = FakeDownloadRepository()
    }

    @Test
    fun `when state is ResolvingPeer and partial data exists, it should transition to Paused`() = runTest {
        val progress = DownloadProgress(
            state = DownloadState.RESOLVING_PEER,
            progressPercentage = 0.5f,
            totalSizeBytes = 1000,
            sourcePeerId = "peer1",
            speedBytesPerSecond = null,
            failureReason = null
        )
        repository.saveDownloadProgress(progress)
        repository.setHasPartialData(true)
        downloadManager = DownloadManager(repository)

        val newProgress = downloadManager.downloadProgress.first()
        assertEquals(DownloadState.PAUSED, newProgress?.state)
    }

    @Test
    fun `when state is ResolvingPeer and no partial data exists, it should transition to Queued`() = runTest {
        val progress = DownloadProgress(
            state = DownloadState.RESOLVING_PEER,
            progressPercentage = 0f,
            totalSizeBytes = 1000,
            sourcePeerId = "peer1",
            speedBytesPerSecond = null,
            failureReason = null
        )
        repository.saveDownloadProgress(progress)
        repository.setHasPartialData(false)
        downloadManager = DownloadManager(repository)

        val newProgress = downloadManager.downloadProgress.first()
        assertEquals(DownloadState.QUEUED, newProgress?.state)
    }

    @Test
    fun `when state is RequestingPermission and partial data exists, it should transition to Paused`() = runTest {
        val progress = DownloadProgress(
            state = DownloadState.REQUESTING_PERMISSION,
            progressPercentage = 0.5f,
            totalSizeBytes = 1000,
            sourcePeerId = "peer1",
            speedBytesPerSecond = null,
            failureReason = null
        )
        repository.saveDownloadProgress(progress)
        repository.setHasPartialData(true)
        downloadManager = DownloadManager(repository)

        val newProgress = downloadManager.downloadProgress.first()
        assertEquals(DownloadState.PAUSED, newProgress?.state)
    }

    @Test
    fun `when state is RequestingPermission and no partial data exists, it should transition to Queued`() = runTest {
        val progress = DownloadProgress(
            state = DownloadState.REQUESTING_PERMISSION,
            progressPercentage = 0f,
            totalSizeBytes = 1000,
            sourcePeerId = "peer1",
            speedBytesPerSecond = null,
            failureReason = null
        )
        repository.saveDownloadProgress(progress)
        repository.setHasPartialData(false)
        downloadManager = DownloadManager(repository)

        val newProgress = downloadManager.downloadProgress.first()
        assertEquals(DownloadState.QUEUED, newProgress?.state)
    }

    @Test
    fun `when state is not ResolvingPeer or RequestingPermission, it should not change`() = runTest {
        val progress = DownloadProgress(
            state = DownloadState.DOWNLOADING,
            progressPercentage = 0.5f,
            totalSizeBytes = 1000,
            sourcePeerId = "peer1",
            speedBytesPerSecond = 100,
            failureReason = null
        )
        repository.saveDownloadProgress(progress)
        downloadManager = DownloadManager(repository)

        val newProgress = downloadManager.downloadProgress.first()
        assertEquals(DownloadState.DOWNLOADING, newProgress?.state)
    }
}
