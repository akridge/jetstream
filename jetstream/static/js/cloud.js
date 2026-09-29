/**
 * JetStream - Cloud Ops Page
 * Cloud-to-cloud sync and generation-safe moves
 */

let cloudTransferJobs = {};
let cloudOperationMode = 'sync';
let moveSourceKind = 'object';
let metadataTimer = null;
let metadataRequestId = 0;
let moveDryRun = false;
const bucketMetadataCache = new Map();

document.addEventListener('DOMContentLoaded', function() {
    setCloudOperationMode('sync');
    loadCloudTransfers();

    // Poll for job updates
    setInterval(loadCloudTransfers, 3000);
});

// ===== GCS PATH HELPER =====

function normalizeGcsPath(path) {
    // Remove any trailing slashes
    path = path.replace(/\/+$/, '');

    // If already starts with gs://, return as-is
    if (path.startsWith('gs://')) {
        return path;
    }

    // Otherwise, add gs:// prefix
    return 'gs://' + path;
}

function getMovePathError(path, label) {
    if (!path.startsWith('gs://')) return `${label} must begin with gs://`;
    const remainder = path.slice(5);
    if (!remainder.includes('/') || !remainder.split('/')[1]) {
        return `${label} must include a bucket and a file or folder`;
    }
    if (isFolderMove() && !path.endsWith('/')) return `${label} must end with / because you are moving a folder`;
    if (!isFolderMove() && path.endsWith('/')) return `${label} looks like a folder. Choose a folder option above to move it.`;
    return '';
}

function isFolderMove() {
    return moveSourceKind === 'folder' || moveSourceKind === 'folder_contents';
}

// FastAPI returns a string detail for handled errors and a list of objects for validation failures.
function extractApiError(data, fallback) {
    const detail = data && data.detail;
    if (typeof detail === 'string' && detail) return detail;
    if (Array.isArray(detail) && detail.length) {
        return detail.map(item => item && item.msg ? item.msg : JSON.stringify(item)).join('; ');
    }
    return fallback;
}

function setCloudOperationMode(mode) {
    cloudOperationMode = mode;
    document.getElementById('sync-mode-button').classList.toggle('active', mode === 'sync');
    document.getElementById('move-mode-button').classList.toggle('active', mode === 'move');
    document.getElementById('sync-options').style.display = mode === 'sync' ? 'block' : 'none';
    document.getElementById('move-options').style.display = mode === 'move' ? 'block' : 'none';
    document.getElementById('operation-description').textContent = mode === 'sync'
        ? 'Sync data between Google Cloud Storage buckets using gcloud storage rsync.'
        : 'Move files between Cloud Storage locations without downloading them. If anything already exists at the destination, nothing is moved.';
    document.getElementById('cloud-operation-submit').textContent = mode === 'sync' ? 'Start Cloud Sync' : 'Move in Cloud Storage';
    document.getElementById('move-dry-run-button').style.display = mode === 'move' ? 'inline-block' : 'none';
    updateMovePathLabels();
    updateCloudPathPreview();
}

function setMoveSourceKind(kind) {
    moveSourceKind = kind;
    document.getElementById('object-kind-button').classList.toggle('active', kind === 'object');
    document.getElementById('folder-kind-button').classList.toggle('active', kind === 'folder');
    document.getElementById('contents-kind-button').classList.toggle('active', kind === 'folder_contents');
    updateMovePathLabels();
    updateCloudPathPreview();
}

function updateMovePathLabels() {
    const isMove = cloudOperationMode === 'move';
    const folderMove = isFolderMove();
    document.getElementById('source-path-label').textContent = isMove
        ? (folderMove ? 'Source folder path' : 'Source file path')
        : 'Source Bucket Path';
    document.getElementById('dest-path-label').textContent = isMove
        ? (folderMove ? 'Destination folder path' : 'Final destination file path')
        : 'Destination Bucket Path';

    const hints = {
        object: 'Moves or renames a single file. The destination path is its final name.',
        folder: 'Moves the folder into the destination, keeping the folder name. Both paths must end with /.',
        folder_contents: 'Moves the files out of the folder into the destination. Use this to rename a folder or merge into an existing one. Both paths must end with /.'
    };
    document.getElementById('move-kind-hint').textContent = hints[moveSourceKind];
}

function renderMoveMappingPreview() {
    const preview = document.getElementById('move-mapping-preview');
    const rows = document.getElementById('move-mapping-rows');
    const note = document.getElementById('move-mapping-note');
    const src = document.getElementById('source-bucket').value.trim();
    const dst = document.getElementById('dest-bucket').value.trim();

    if (cloudOperationMode !== 'move' || !src || !dst) {
        preview.style.display = 'none';
        return;
    }

    const pathError = getMovePathError(src, 'Source') || getMovePathError(dst, 'Destination');
    if (pathError) {
        rows.textContent = '';
        note.textContent = pathError;
        preview.style.display = 'block';
        return;
    }

    if (moveSourceKind === 'object') {
        rows.textContent = `${src}  \u2192  ${dst}`;
        note.textContent = 'The source file is deleted only after the copy is verified at the destination.';
    } else {
        const sourceFolder = src.endsWith('/') ? src : src + '/';
        const destBase = dst.endsWith('/') ? dst : dst + '/';
        const sourceName = sourceFolder.replace(/\/+$/, '').split('/').pop();
        const destFolder = moveSourceKind === 'folder' ? `${destBase}${sourceName}/` : destBase;
        rows.innerHTML = '';
        ['report.csv', '2024/logs.txt'].forEach(function(relativeName) {
            const line = document.createElement('div');
            line.textContent = `${sourceFolder}${relativeName}  \u2192  ${destFolder}${relativeName}`;
            rows.appendChild(line);
        });
        note.textContent = moveSourceKind === 'folder'
            ? `The folder "${sourceName}" is created inside the destination. Source files are deleted only after every copy is verified.`
            : `Files land directly in the destination, so no "${sourceName}" folder is created inside it. Source files are deleted only after every copy is verified; an empty folder placeholder can remain behind.`;
    }

    preview.style.display = 'block';
}

// ===== PATH PREVIEW =====

function updateCloudPathPreview() {
    const src = document.getElementById('source-bucket').value.trim();
    const dst = document.getElementById('dest-bucket').value.trim();

    const srcDiv = document.getElementById('source-path-preview');
    const dstDiv = document.getElementById('dest-path-preview');

    if (src) {
        const norm = cloudOperationMode === 'move' ? src : normalizeGcsPath(src) + '/';
        document.getElementById('source-path-preview-text').textContent = norm;
        srcDiv.style.display = 'block';
    } else {
        srcDiv.style.display = 'none';
    }

    if (dst) {
        const norm = cloudOperationMode === 'move' ? dst : normalizeGcsPath(dst) + '/';
        document.getElementById('dest-path-preview-text').textContent = norm;
        dstDiv.style.display = 'block';
    } else {
        dstDiv.style.display = 'none';
    }

    scheduleBucketMetadataLookup();
    renderMoveMappingPreview();
}

function scheduleBucketMetadataLookup() {
    clearTimeout(metadataTimer);
    const requestId = ++metadataRequestId;
    if (cloudOperationMode !== 'move') {
        document.getElementById('source-bucket-metadata').textContent = '';
        document.getElementById('dest-bucket-metadata').textContent = '';
        document.getElementById('move-cost-warning').style.display = 'none';
        return;
    }
    const sourcePath = document.getElementById('source-bucket').value.trim();
    const destPath = document.getElementById('dest-bucket').value.trim();
    if (!sourcePath || !destPath) {
        document.getElementById('source-bucket-metadata').textContent = '';
        document.getElementById('dest-bucket-metadata').textContent = '';
        document.getElementById('move-cost-warning').style.display = 'none';
        return;
    }
    metadataTimer = setTimeout(() => loadMoveMetadata(requestId), 600);
}

async function fetchBucketMetadata(path) {
    const bucket = path.startsWith('gs://') ? path.slice(5).split('/')[0] : '';
    if (!bucket) throw new Error('Enter a gs:// bucket path');
    if (bucketMetadataCache.has(bucket)) return bucketMetadataCache.get(bucket);

    const response = await fetch(`/api/cloud/bucket-metadata?path=${encodeURIComponent(`gs://${bucket}`)}`);
    const data = await response.json();
    if (!response.ok) throw new Error(extractApiError(data, 'Metadata unavailable'));
    bucketMetadataCache.set(bucket, data);
    return data;
}

function formatBucketMetadata(metadata) {
    const locations = metadata.data_locations && metadata.data_locations.length
        ? metadata.data_locations.join('/')
        : (metadata.location || 'unknown');
    return `Location: ${locations} | Default storage: ${metadata.storage_class || 'unknown'}`;
}

function getLocationKey(metadata) {
    if (!metadata) return '';
    const locations = metadata.data_locations && metadata.data_locations.length
        ? [...metadata.data_locations].sort()
        : [metadata.location];
    return locations.filter(Boolean).map(value => value.toUpperCase()).join('|');
}

async function loadMoveMetadata(requestId) {
    const sourcePath = document.getElementById('source-bucket').value.trim();
    const destPath = document.getElementById('dest-bucket').value.trim();
    if (!sourcePath || !destPath) return;

    const results = await Promise.allSettled([
        fetchBucketMetadata(sourcePath),
        fetchBucketMetadata(destPath)
    ]);
    if (requestId !== metadataRequestId || cloudOperationMode !== 'move') return;

    const sourceMetadata = results[0].status === 'fulfilled' ? results[0].value : null;
    const destMetadata = results[1].status === 'fulfilled' ? results[1].value : null;
    document.getElementById('source-bucket-metadata').textContent = sourceMetadata
        ? formatBucketMetadata(sourceMetadata)
        : `Metadata unavailable: ${results[0].reason.message}`;
    document.getElementById('dest-bucket-metadata').textContent = destMetadata
        ? formatBucketMetadata(destMetadata)
        : `Metadata unavailable: ${results[1].reason.message}`;
    renderMoveCostWarning(sourceMetadata, destMetadata);
}

function renderMoveCostWarning(sourceMetadata, destMetadata) {
    const warning = document.getElementById('move-cost-warning');
    const reasons = [];
    const sourceLocation = getLocationKey(sourceMetadata);
    const destLocation = getLocationKey(destMetadata);
    const sourceClass = (sourceMetadata && sourceMetadata.storage_class || '').toUpperCase();

    if (!sourceLocation || !destLocation) reasons.push('One or both bucket locations are unknown; network charges may apply.');
    else if (sourceLocation !== destLocation) reasons.push('Buckets use different locations; network transfer charges may apply.');
    if (['NEARLINE', 'COLDLINE', 'ARCHIVE', 'ARCHIVAL'].includes(sourceClass)) {
        reasons.push(`Source default storage is ${sourceClass}; retrieval or early-deletion charges may apply.`);
    }

    warning.className = `dc-alert ${reasons.length ? 'warning' : 'success'}`;
    warning.textContent = reasons.length ? reasons.join(' ') : 'Bucket locations match and no retrieval-sensitive source default was detected.';
    warning.style.display = 'block';
}

// ===== ADVANCED OPTIONS =====

function toggleAdvancedOptions() {
    const advancedOptions = document.getElementById('advanced-options');
    const toggleIcon = document.getElementById('advanced-toggle');

    if (advancedOptions.style.display === 'none') {
        advancedOptions.style.display = 'block';
        toggleIcon.textContent = '\u25b2';
    } else {
        advancedOptions.style.display = 'none';
        toggleIcon.textContent = '\u25bc';
    }
}

// ===== FORM SUBMIT =====

function startMoveDryRun() {
    moveDryRun = true;
    document.getElementById('cloud-transfer-form').requestSubmit();
}

document.getElementById('cloud-transfer-form').addEventListener('submit', async function(e) {
    e.preventDefault();

    const isMoveDryRun = moveDryRun;
    moveDryRun = false;

    const sourceBucket = document.getElementById('source-bucket').value.trim();
    const destBucket = document.getElementById('dest-bucket').value.trim();
    const recursive = document.getElementById('recursive').checked;
    const dryRun = document.getElementById('dry-run').checked;
    const noClobber = document.getElementById('no-clobber').checked;
    const excludePatterns = document.getElementById('exclude-patterns').value
        .split('\n')
        .map(s => s.trim())
        .filter(s => s.length > 0);

    if (!sourceBucket || !destBucket) {
        showToast('Please enter both source and destination paths', 'error');
        return;
    }

    if (cloudOperationMode === 'move') {
        const sourceError = getMovePathError(sourceBucket, 'Source');
        const destError = getMovePathError(destBucket, 'Destination');
        if (sourceError || destError) {
            showToast(sourceError || destError, 'error');
            return;
        }
    }

    const submitBtn = this.querySelector('button[type="submit"]');
    const dryRunBtn = document.getElementById('move-dry-run-button');
    const activeBtn = isMoveDryRun ? dryRunBtn : submitBtn;
    const activeBtnText = activeBtn.textContent;
    submitBtn.disabled = true;
    dryRunBtn.disabled = true;
    activeBtn.textContent = cloudOperationMode === 'sync'
        ? 'Starting Sync...'
        : (isMoveDryRun ? 'Checking...' : 'Starting Move...');

    try {
        const endpoint = cloudOperationMode === 'sync' ? '/api/cloud/transfer' : '/api/cloud/move';
        const payload = cloudOperationMode === 'sync' ? {
            source_path: sourceBucket,
            dest_path: destBucket,
            recursive: recursive,
            dry_run: dryRun,
            no_clobber: noClobber,
            exclude_patterns: excludePatterns
        } : {
            source_path: sourceBucket,
            dest_path: destBucket,
            source_kind: moveSourceKind,
            dry_run: isMoveDryRun
        };
        const response = await fetch(endpoint, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(payload)
        });

        const data = await response.json();

        if (response.ok) {
            const message = cloudOperationMode === 'move'
                ? (isMoveDryRun ? 'Dry run started. Nothing will be copied or deleted.' : 'Move started successfully.')
                : (dryRun ? 'Dry run started. Check results below.' : 'Sync started successfully.');
            showToast(message, 'success');
            loadCloudTransfers();
        } else {
            showToast(extractApiError(data, `Failed to start ${cloudOperationMode}`), 'error');
        }
    } catch (error) {
        console.error('Error starting cloud operation:', error);
        showToast('Error starting cloud operation: ' + error.message, 'error');
    } finally {
        submitBtn.disabled = false;
        dryRunBtn.disabled = false;
        activeBtn.textContent = activeBtnText;
    }
});

// ===== LOAD CLOUD TRANSFERS =====

async function loadCloudTransfers() {
    try {
        const response = await fetch('/api/cloud/transfers');
        if (!response.ok) {
            const container = document.getElementById('cloud-jobs-list');
            container.innerHTML = '<p style="text-align:center;color:#dc2626;padding:30px 0;">Failed to load transfers (API error ' + response.status + ')</p>';
            return;
        }

        const jobs = await response.json();
        cloudTransferJobs = {};
        jobs.forEach(job => {
            cloudTransferJobs[job.job_id] = job;
        });

        renderCloudJobs(jobs);
    } catch (error) {
        console.error('Error loading transfers:', error);
        const container = document.getElementById('cloud-jobs-list');
        container.innerHTML = '<p style="text-align:center;color:#dc2626;padding:30px 0;">Could not connect to server.</p>';
    }
}

function renderCloudJobs(jobs) {
    const container = document.getElementById('cloud-jobs-list');

    // Update stats
    updateTransferStats(jobs);

    const visibleJobs = jobs;

    if (!visibleJobs || visibleJobs.length === 0) {
        container.innerHTML = '<p style="text-align: center; color: #999; padding: 50px 0;">No cloud operations yet. Use the form to start one.</p>';
        return;
    }

    // Sort by created_at descending (newest first)
    visibleJobs.sort((a, b) => new Date(b.created_at) - new Date(a.created_at));

    // Update last updated timestamp
    const lastUpdated = document.getElementById('transfers-last-updated');
    if (lastUpdated) {
        lastUpdated.textContent = `(updated ${new Date().toLocaleTimeString()})`;
    }

    container.innerHTML = visibleJobs.map(job => {
        const statusClass = getStatusClass(job.status);
        const statusIcon = getStatusIcon(job.status);
        const isDryRun = job.dry_run ? ' (Dry Run)' : '';
        const operation = job.operation === 'move' ? 'Move' : 'Sync';
        const phase = job.operation === 'move' && job.status === 'running' && job.phase
            ? ` | Phase: ${escapeHtml(job.phase)}`
            : '';

        // Shorten paths for display
        const sourcePath = shortenPath(job.source_path);
        const destPath = shortenPath(job.dest_path);

        const isClickable = job.status === 'completed' || job.status === 'failed';

        return `
            <div class="job-item ${statusClass}" ${isClickable ? `onclick="viewTransferOutput('${job.job_id}')" style="cursor: pointer;"` : ''}>
                <div class="job-info">
                    <div class="job-name">
                        ${statusIcon} <span class="job-badge badge-rsync">${operation.toUpperCase()}</span> ${escapeHtml(sourcePath)} → ${escapeHtml(destPath)}${isDryRun}
                    </div>
                    <div class="job-meta" style="font-size: 0.85em; color: #64748b;">
                        Started: ${formatDate(job.created_at)}
                        ${job.completed_at ? ' | Completed: ' + formatDate(job.completed_at) : ''}${phase}
                        ${job.operation === 'move' && job.total_files ? ` | ${job.files_uploaded || 0} / ${job.total_files} verified` : ''}
                    </div>
                    ${job.error ? `<div style="color: #dc2626; font-size: 0.85em; margin-top: 4px;">Error: ${escapeHtml(job.error)}</div>` : ''}
                </div>
                <div class="job-actions">
                    <span class="status-badge ${statusClass}">${job.status}</span>
                </div>
            </div>
        `;
    }).join('');
}

function shortenPath(path) {
    if (!path) return 'Unknown';
    // Remove gs:// prefix for cleaner display
    path = path.replace(/^gs:\/\//, '');
    // If path is too long, truncate middle
    if (path.length > 40) {
        return path.substring(0, 20) + '...' + path.substring(path.length - 17);
    }
    return path;
}

function updateTransferStats(jobs) {
    const total = jobs.length;
    const active = jobs.filter(j => j.status === 'running').length;
    const completed = jobs.filter(j => j.status === 'completed').length;
    const failed = jobs.filter(j => j.status === 'failed').length;

    document.getElementById('total-transfers').textContent = total;
    document.getElementById('active-transfers').textContent = active;
    document.getElementById('completed-transfers').textContent = completed;
    document.getElementById('failed-transfers').textContent = failed;
}

function getStatusClass(status) {
    switch (status) {
        case 'running': return 'status-running';
        case 'completed': return 'status-completed';
        case 'failed': return 'status-failed';
        default: return 'status-pending';
    }
}

function getStatusIcon(status) {
    switch (status) {
        case 'running': return '⏳';
        case 'completed': return '✅';
        case 'failed': return '❌';
        default: return '⏸️';
    }
}

function formatCloudCommand(command) {
    if (!command) return 'N/A';
    return command.replace(/^.*?(?:^|[\\/])gcloud(?:\.cmd|\.exe|\.bat)?(?=\s)/i, 'gcloud');
}

function formatCloudOutput(output) {
    if (!output) return '';
    return output
        .split('\n')
        .filter(line => !line.includes('The following characters are invalid in Windows file and directory names'))
        .filter(line => !/^Renaming [0-9a-f]+\.v_.* to [0-9a-f]+\.v_/i.test(line.trim()))
        .join('\n')
        .trim();
}

// ===== VIEW TRANSFER OUTPUT =====

async function viewTransferOutput(jobId) {
    ensureModalExists();
    try {
        const response = await fetch(`/api/cloud/transfer/${jobId}`);
        if (!response.ok) throw new Error('Failed to fetch transfer details');

        const job = await response.json();

        const modalBody = document.getElementById('modalBody');
        let html = '';
        
        const operation = job.operation === 'move' ? 'Move' : 'Sync';
        html += `<div class="detail-section"><h3>Status</h3><span class="status-badge ${getStatusClass(job.status)}">${job.status.toUpperCase()}</span> <span class="job-badge badge-rsync">${operation.toUpperCase()}</span>${job.dry_run ? ' <span class="job-badge badge-dry-run">DRY-RUN</span>' : ''}</div>`;

        if (job.operation !== 'move') {
            html += `<div class="detail-section"><h3>Command</h3><div class="detail-value">${escapeHtml(formatCloudCommand(job.command))}</div></div>`;
        }

        html += '<div class="detail-section"><h3>Transfer Information</h3><div class="detail-grid">';
        html += `<div class="detail-item"><span class="detail-label">Job ID</span><span class="detail-text">${job.job_id}</span></div>`;
        html += `<div class="detail-item"><span class="detail-label">Created</span><span class="detail-text">${formatDate(job.created_at)}</span></div>`;
        html += `<div class="detail-item"><span class="detail-label">Completed</span><span class="detail-text">${formatDate(job.completed_at)}</span></div>`;
        if (job.operation === 'move') {
            html += `<div class="detail-item"><span class="detail-label">Phase</span><span class="detail-text">${escapeHtml(job.phase || 'N/A')}</span></div>`;
            html += `<div class="detail-item"><span class="detail-label">Verified</span><span class="detail-text">${job.files_uploaded || 0} / ${job.total_files || 0} objects</span></div>`;
        }
        html += '</div></div>';

        html += '<div class="detail-section"><h3>Source & Destination</h3><div class="detail-grid">';
        html += `<div class="detail-item"><span class="detail-label">Source Path</span><span class="detail-text" style="word-break: break-word; overflow-wrap: anywhere;">${escapeHtml(job.source_path)}</span></div>`;
        html += `<div class="detail-item"><span class="detail-label">Destination Path</span><span class="detail-text" style="word-break: break-word; overflow-wrap: anywhere;">${escapeHtml(job.dest_path)}</span></div>`;
        html += '</div></div>';

        if (job.error) {
            html += `<div class="detail-section"><h3>Error Message</h3><div class="detail-value" style="color: #991b1b; background: #fee2e2;">${escapeHtml(job.error)}</div></div>`;
        }

        const cloudOutput = formatCloudOutput(job.output);
        if (cloudOutput) {
            html += '<div class="detail-section"><h3>Command Output</h3>';
            html += `<div class="detail-value" style="max-height: 300px; overflow-y: auto; white-space: pre-wrap; word-wrap: break-word; overflow-wrap: anywhere;">${escapeHtml(cloudOutput)}</div></div>`;
        }

        html += '<div class="modal-actions">';
        html += '<button class="btn btn-secondary" onclick="closeJobModal()">Close</button>';
        html += '</div>';

        modalBody.innerHTML = html;
        document.getElementById('jobModal').style.display = 'block';
    } catch (error) {
        console.error('Error fetching transfer details:', error);
        showToast('Error loading transfer details', 'error');
    }
}

// ===== CLEAR COMPLETED TRANSFERS =====

async function clearCompletedTransfers() {
    if (!confirm('This will hide all completed and failed cloud operations from the recent list. Continue?')) {
        return;
    }

    try {
        const response = await fetch('/api/cloud/transfers/clear-completed', { method: 'POST' });
        const data = await response.json();
        if (!response.ok) {
            showToast(data.detail || 'Failed to clear transfers', 'error');
            return;
        }

        await loadCloudTransfers();
        showToast(data.message || 'Completed transfers hidden from view', 'success');
    } catch (error) {
        console.error('Error clearing transfers:', error);
        showToast('Failed to clear transfers', 'error');
    }
}
