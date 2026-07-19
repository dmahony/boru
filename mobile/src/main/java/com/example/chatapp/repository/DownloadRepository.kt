package com.example.chatapp.repository

import com.example.chatapp.model.DownloadProgress

interface DownloadRepository {
    fun getDownloadProgress(): DownloadProgress?
    fun saveDownloadProgress(downloadProgress: DownloadProgress)
    fun hasPartialData(): Boolean
}
