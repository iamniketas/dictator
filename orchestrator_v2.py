# orchestrator_v2.py
# Enhanced Task Orchestration Daemon with Background Mode & Watermarking
# Централизованный оркестратор для изоляции Claude от кода других агентов

from fastapi import FastAPI, HTTPException, BackgroundTasks
from fastapi.responses import JSONResponse
from pydantic import BaseModel
from datetime import datetime
import sqlite3
import subprocess
import json
import asyncio
from pathlib import Path
import threading
import requests
from typing import Optional, Dict, Any, List
import logging
import time
import hashlib

# Настройка логирования
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
)
logger = logging.getLogger(__name__)

app = FastAPI(title="Task Orchestration Daemon v2")

# GLM Context Management Constants
GLM_MAX_CONTEXT = 32000
GLM_CONTEXT_WARNING = 28000
GLM_CONTEXT_CRITICAL = 30000
TOKEN_ESTIMATION_FACTOR = 4
SUMMARIZATION_THRESHOLD = 4000
SUBTASK_OVERLAP = 0.8

# Конфигурация
DB_PATH = "orchestrator.db"
GLM_MODEL = "glm-4.7-flash"
KIMI_API_KEY = "sk-kimi-QjfnjrhxhV9Z7gmCLnl0L581a9eZsv90aguRg6Ha0fHqFjw74BggXOHkXow2BUhY"
KIMI_API_URL = "https://api.kimi.com/coding/v1/messages"
KIMI_MODEL = "kimi-for-coding"

# Директории для результатов
RESULTS_DIR = Path(".orchestrator")
GLM_DIR = RESULTS_DIR / "glm"
KIMI_DIR = RESULTS_DIR / "kimi"
OPUS_DIR = RESULTS_DIR / "opus"
REPORTS_DIR = RESULTS_DIR / "reports"

# Создаем директории
for d in [GLM_DIR, KIMI_DIR, OPUS_DIR, REPORTS_DIR]:
    d.mkdir(parents=True, exist_ok=True)

# In-memory task status tracking for background tasks
task_progress = {}

def add_agent_watermark(code: str, agent_type: str, task_id: str, file_path: str = "") -> str:
    """Add agent watermark to generated code with appropriate comment syntax"""
    timestamp = datetime.utcnow().isoformat()

    # Determine comment syntax based on file extension
    if file_path.endswith('.toml'):
        comment = '#'
    else:
        comment = '//'

    watermark = f"{comment} AGENT: {agent_type} | TASK: {task_id} | TIMESTAMP: {timestamp}\n"
    watermark += f"{comment} AUTO-GENERATED: Do not edit manually. Delegate changes via orchestrator.\n"
    watermark += f"{comment} SOURCE: http://localhost:8000/task/{task_id}/report\n\n"

    # If code already starts with comments, replace them
    if code.strip().startswith(("//", "#")):
        lines = code.split('\n')
        # Skip consecutive comment lines at start
        i = 0
        while i < len(lines) and lines[i].strip().startswith(("//", "#")):
            i += 1
        code = '\n'.join(lines[i:])

    return watermark + code

def verify_agent_watermark(file_path: Path) -> Dict[str, Any]:
    """Verify that file has agent watermark"""
    try:
        with open(file_path, 'r', encoding='utf-8') as f:
            first_line = f.readline().strip()

        # Check pattern: // AGENT: {agent} | TASK: {task_id} | TIMESTAMP: {iso}
        parts = first_line.split(' | ')
        if len(parts) >= 3:
            agent_part = parts[0].replace('// AGENT: ', '')
            task_part = parts[1].replace('TASK: ', '')
            time_part = parts[2].replace('TIMESTAMP: ', '')

            return {
                "verified": True,
                "agent": agent_part,
                "task_id": task_part,
                "timestamp": time_part
            }

        return {"verified": False, "error": "Invalid watermark format"}
    except Exception as e:
        return {"verified": False, "error": str(e)}

# GLM Context Management Module
def count_tokens(text: str) -> int:
    return len(text) // TOKEN_ESTIMATION_FACTOR

def count_file_tokens(file_path: Path) -> int:
    try:
        with open(file_path, 'r', encoding='utf-8') as f:
            return count_tokens(f.read())
    except Exception as e:
        logger.warning(f"Could not count tokens for {file_path}: {e}")
        return 0

def summarize_file(file_path: Path, max_chars: int = 3000) -> str:
    try:
        with open(file_path, 'r', encoding='utf-8') as f:
            content = f.read()

        if len(content) <= max_chars:
            return content

        lines = content.split('\n')
        summarized = []
        in_imports = False
        in_function = False
        function_indent = 0

        for line in lines:
            stripped = line.lstrip()

            if stripped.startswith('use ') or stripped.startswith('mod '):
                in_imports = True
                summarized.append(line)
                continue

            if stripped.startswith('pub fn ') or stripped.startswith('fn '):
                in_function = True
                function_indent = len(line) - len(stripped)
                summarized.append(line)
                continue

            if in_function:
                current_indent = len(line) - len(line.lstrip())
                if current_indent <= function_indent and stripped:
                    in_function = False
                    summarized.append(line)
                elif len('\n'.join(summarized)) > max_chars:
                    break
                else:
                    summarized.append(line)
            elif in_imports:
                if len('\n'.join(summarized)) > max_chars:
                    break
                summarized.append(line)
            else:
                if len('\n'.join(summarized)) > max_chars:
                    break
                summarized.append(line)

        return '\n'.join(summarized)
    except Exception as e:
        logger.error(f"Error summarizing {file_path}: {e}")
        return f"# Error reading file: {file_path}\n{str(e)}"

def prepare_glm_context(spec: Dict[str, Any], context_files: list[str]) -> str:
    file_contents = {}
    for file_path_str in context_files:
        file_path = Path(file_path_str)
        if file_path.exists():
            file_contents[file_path] = file_path.read_text(encoding='utf-8')
        else:
            logger.warning(f"Context file not found: {file_path}")

    total_tokens = sum(count_file_tokens(fp) for fp in file_contents.keys())
    logger.info(f"Context size: {total_tokens} tokens (max: {GLM_MAX_CONTEXT})")

    if total_tokens < GLM_CONTEXT_WARNING:
        logger.info("Using full context (under warning threshold)")
        context_text = "\n\n".join([
            f"# {fp}\n{content}"
            for fp, content in file_contents.items()
        ])
        return context_text

    if total_tokens < GLM_CONTEXT_CRITICAL:
        logger.info("Using summarization (over warning, under critical)")
        summarized_files = []

        for fp, content in file_contents.items():
            file_tokens = count_file_tokens(fp)
            if file_tokens > SUMMARIZATION_THRESHOLD:
                logger.info(f"Summarizing {fp} ({file_tokens} tokens)")
                summary = summarize_file(fp)
                summarized_files.append(f"# {fp} (SUMMARIZED)\n{summary}")
            else:
                summarized_files.append(f"# {fp}\n{content}")

        context_text = "\n\n".join(summarized_files)
        return context_text

    logger.warning(f"Context over critical threshold ({total_tokens} tokens), splitting into subtasks")
    subtask_dir = GLM_DIR / "context" / spec.get('task_id', 'unknown')
    subtask_dir.mkdir(parents=True, exist_ok=True)

    subtask_results = []
    remaining_files = list(file_contents.items())

    while remaining_files:
        batch_tokens = sum(count_file_tokens(fp) for fp, _ in remaining_files)
        batch_size = len(remaining_files)

        if batch_tokens < GLM_CONTEXT_WARNING:
            batch_files = remaining_files
            remaining_files = []
        else:
            batch_files = []
            temp_tokens = 0
            for fp, content in remaining_files:
                file_tokens = count_file_tokens(fp)
                if temp_tokens + file_tokens < GLM_CONTEXT_WARNING:
                    batch_files.append((fp, content))
                    temp_tokens += file_tokens
                else:
                    break
            remaining_files = remaining_files[len(batch_files):]

        subtask_context = "\n\n".join([
            f"# {fp}\n{content}"
            for fp, content in batch_files
        ])

        subtask_id = f"{spec.get('task_id', 'unknown')}_subtask_{len(subtask_results) + 1}"
        subtask_file = subtask_dir / f"{subtask_id}.txt"
        subtask_file.write_text(subtask_context, encoding='utf-8')

        subtask_results.append({
            "subtask_id": subtask_id,
            "file_count": len(batch_files),
            "tokens": batch_tokens,
            "file_path": str(subtask_file)
        })

        logger.info(f"Created subtask {len(subtask_results)}: {subtask_id} ({batch_tokens} tokens, {len(batch_files)} files)")

    subtask_info = f"""# CONTEXT SPLIT INTO {len(subtask_results)} SUBTASKS

GLM context exceeds {GLM_MAX_CONTEXT} token limit. Task split into sequential subtasks:

"""
    for i, result in enumerate(subtask_results, 1):
        subtask_info += f"""
## Subtask {i}: {result['subtask_id']}
- Files: {result['file_count']}
- Tokens: {result['tokens']}
- Context file: {result['file_path']}

"""

    subtask_info += """## Instructions
Execute subtasks sequentially. After completing each subtask, save results to the appropriate file path specified in the contract.

"""
    return subtask_info

# Pydantic models
class TaskSpec(BaseModel):
    task: str
    context_files: list[str]
    acceptance_criteria: list[str]
    validation_command: Optional[str] = None
    output_file: Optional[str] = None
    agent_type: str = "glm"  # glm, kimi, opus

class TaskCreateResponse(BaseModel):
    task_id: str
    status: str
    message: str

class TaskStatusResponse(BaseModel):
    task_id: str
    status: str  # pending, running, completed, failed
    agent_type: Optional[str]
    progress_percent: int
    current_step: Optional[str]
    validation_report: Optional[Dict[str, Any]]
    error: Optional[str]
    started_at: Optional[str]
    completed_at: Optional[str]

class TaskReportResponse(BaseModel):
    task_id: str
    status: str
    agent_type: str
    summary: str
    detailed_log: List[Dict[str, Any]]
    files_generated: List[str]
    validation_results: Optional[Dict[str, Any]]
    agent_watermarks: List[Dict[str, Any]]
    time_elapsed_seconds: int

# Database operations
def init_db():
    conn = sqlite3.connect(DB_PATH)
    cursor = conn.cursor()

    # Main tasks table
    cursor.execute("""
        CREATE TABLE IF NOT EXISTS tasks (
            id TEXT PRIMARY KEY,
            agent_type TEXT,
            status TEXT,
            spec_path TEXT,
            result_path TEXT,
            validation_report TEXT,
            error TEXT,
            progress_percent INTEGER DEFAULT 0,
            current_step TEXT,
            started_at TIMESTAMP,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            completed_at TIMESTAMP
        )
    """)

    # Task logs table for detailed reporting
    cursor.execute("""
        CREATE TABLE IF NOT EXISTS task_logs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            task_id TEXT,
            timestamp TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            level TEXT,  -- INFO, WARNING, ERROR, SUCCESS
            step TEXT,
            message TEXT,
            FOREIGN KEY (task_id) REFERENCES tasks(id)
        )
    """)

    conn.commit()
    conn.close()
    logger.info("Database initialized")

def generate_task_id() -> str:
    import uuid
    return f"task_{uuid.uuid4().hex[:12]}"

def save_spec(task_id: str, spec: TaskSpec) -> str:
    spec_path = RESULTS_DIR / "specs" / f"{task_id}.json"
    spec_path.parent.mkdir(parents=True, exist_ok=True)

    with open(spec_path, 'w', encoding='utf-8') as f:
        json.dump(spec.model_dump(), f, indent=2, ensure_ascii=False)

    return str(spec_path)

def log_task_event(task_id: str, level: str, step: str, message: str):
    """Log event to database and memory"""
    conn = sqlite3.connect(DB_PATH)
    cursor = conn.cursor()
    cursor.execute("""
        INSERT INTO task_logs (task_id, level, step, message)
        VALUES (?, ?, ?, ?)
    """, (task_id, level, step, message))
    conn.commit()
    conn.close()

    # Also update in-memory progress
    if task_id not in task_progress:
        task_progress[task_id] = {"logs": []}
    task_progress[task_id]["logs"].append({
        "timestamp": datetime.utcnow().isoformat(),
        "level": level,
        "step": step,
        "message": message
    })

def update_task_status(task_id: str, status: str, progress: int = None, step: str = None, error: str = None):
    conn = sqlite3.connect(DB_PATH)
    cursor = conn.cursor()

    fields = ["status = ?"]
    values = [status]

    if progress is not None:
        fields.append("progress_percent = ?")
        values.append(progress)
    if step:
        fields.append("current_step = ?")
        values.append(step)
    if error:
        fields.append("error = ?")
        values.append(error)

    if status in ["completed", "failed"]:
        fields.append("completed_at = CURRENT_TIMESTAMP")
    elif status == "running":
        fields.append("started_at = CURRENT_TIMESTAMP")

    values.append(task_id)

    query = f"UPDATE tasks SET {', '.join(fields)} WHERE id = ?"
    cursor.execute(query, values)
    conn.commit()
    conn.close()

# Background task execution
async def run_agent_background(task_id: str, spec_path: str, agent_type: str):
    """Run agent task in background with progress tracking"""
    spec = json.load(open(spec_path, encoding='utf-8'))

    log_task_event(task_id, "INFO", "INIT", f"Starting {agent_type} background task")
    update_task_status(task_id, "running", progress=0, step="Initializing")

    try:
        if agent_type == "glm":
            await run_glm_background(task_id, spec)
        elif agent_type == "kimi":
            await run_kimi_background(task_id, spec)
        else:
            raise ValueError(f"Unknown agent type: {agent_type}")
    except Exception as e:
        log_task_event(task_id, "ERROR", "EXECUTION", str(e))
        update_task_status(task_id, "failed", error=str(e))

async def run_glm_background(task_id: str, spec: Dict[str, Any]):
    """GLM background execution with detailed logging"""
    output_file = spec.get('output_file', f'src/generated_{task_id}.rs')
    result_path = Path(output_file)

    # Step 1: Context preparation
    log_task_event(task_id, "INFO", "CONTEXT", "Preparing context with 32K window management")
    update_task_status(task_id, "running", progress=10, step="Preparing context")
    await asyncio.sleep(0.1)  # Allow other tasks to run

    context_text = prepare_glm_context(spec, spec.get('context_files', []))

    # Step 2: API call
    log_task_event(task_id, "INFO", "GENERATION", "Calling GLM via Ollama API")
    update_task_status(task_id, "running", progress=30, step="Generating code")

    prompt = f"""Ты — Rust разработчик. Выполни задачу.

## Задача
{spec.get('task', 'No task specified')}

## Контекстные файлы
{chr(10).join(f"- {f}" for f in spec.get('context_files', []))}

## Критерии приемки
{chr(10).join(f"- {c}" for c in spec.get('acceptance_criteria', []))}

## Контекст
{context_text}

## Требования
1. Пиши код ТОЛЬКО в один файл
2. Не объясняй, не чатай — ТОЛЬКО код
3. Используй стандартные идиомы Rust
4. Добавь необходимые imports

Выведи ТОЛЬКО готовый код без markdown блоков.
"""

    try:
        response = requests.post(
            "http://localhost:11434/api/chat",
            json={
                "model": GLM_MODEL,
                "messages": [
                    {"role": "system", "content": "You are a Rust code generator. Output ONLY valid Rust code. No explanations. No markdown."},
                    {"role": "user", "content": prompt}
                ],
                "stream": False,
                "options": {"num_ctx": GLM_MAX_CONTEXT}
            },
            timeout=300
        )

        if response.status_code != 200:
            raise Exception(f"Ollama API error: {response.status_code}")

        # Step 3: Process and watermark
        log_task_event(task_id, "INFO", "WATERMARK", "Adding agent watermark to generated code")
        update_task_status(task_id, "running", progress=60, step="Processing output")

        code = response.json()['message']['content']
        watermarked_code = add_agent_watermark(code, "glm", task_id, output_file)

        # Step 4: Save file
        result_path.parent.mkdir(parents=True, exist_ok=True)
        with open(result_path, 'w', encoding='utf-8') as f:
            f.write(watermarked_code)

        log_task_event(task_id, "SUCCESS", "SAVE", f"Code saved to {output_file}")
        update_task_status(task_id, "running", progress=70, step="File saved")

        # Step 5: Validation
        log_task_event(task_id, "INFO", "VALIDATION", "Running validation commands")
        update_task_status(task_id, "running", progress=80, step="Validating")

        validation = run_validation(task_id, spec.get('validation_command'))

        # Step 6: Finalize
        status = "completed" if validation['passed'] else "failed"
        log_task_event(task_id, "SUCCESS" if validation['passed'] else "WARNING",
                       "COMPLETE", f"Task completed with status: {status}")

        conn = sqlite3.connect(DB_PATH)
        cursor = conn.cursor()
        cursor.execute("""
            UPDATE tasks
            SET status = ?, validation_report = ?, result_path = ?, completed_at = CURRENT_TIMESTAMP
            WHERE id = ?
        """, (status, json.dumps(validation), str(result_path), task_id))
        conn.commit()
        conn.close()

        update_task_status(task_id, status, progress=100, step="Complete")

    except Exception as e:
        log_task_event(task_id, "ERROR", "GENERATION", str(e))
        update_task_status(task_id, "failed", error=str(e))
        raise

async def run_kimi_background(task_id: str, spec: Dict[str, Any]):
    """Kimi background execution"""
    output_file = spec.get('output_file', f'src/generated_{task_id}.rs')
    result_path = Path(output_file)

    log_task_event(task_id, "INFO", "CONTEXT", "Preparing context for Kimi K2.5")
    update_task_status(task_id, "running", progress=10, step="Preparing context")

    context_text = prepare_glm_context(spec, spec.get('context_files', []))

    log_task_event(task_id, "INFO", "GENERATION", "Calling Kimi K2.5 API")
    update_task_status(task_id, "running", progress=30, step="Generating code")

    system_message = "You are a Rust code generator. You write code ONCE and save it to file. Do not explain. Do not chat. Execute the task. Output ONLY code without markdown code blocks."

    user_prompt = f"""Ты — Rust разработчик. Выполни задачу.

## Задача
{spec.get('task', '')}

## Контекстные файлы
{chr(10).join(f"- {f}" for f in spec.get('context_files', []))}

## Контекст
{context_text}

## Требования
1. Пиши код ТОЛЬКО в файлы, указанные в контексте
2. Не объясняй, не чатай
3. Используй стандартные идиомы Rust
4. Соблюдай критерии приемки

Выведи ТОЛЬКО готовый код без markdown блоков.
"""

    try:
        response = requests.post(
            KIMI_API_URL,
            headers={
                "x-api-key": KIMI_API_KEY,
                "anthropic-version": "2023-06-01",
                "content-type": "application/json"
            },
            json={
                "model": KIMI_MODEL,
                "max_tokens": 32768,
                "system": system_message,
                "messages": [{"role": "user", "content": user_prompt}]
            },
            timeout=300
        )

        if response.status_code != 200:
            raise Exception(f"Kimi API error: {response.status_code}")

        log_task_event(task_id, "INFO", "WATERMARK", "Adding agent watermark")
        update_task_status(task_id, "running", progress=60, step="Processing output")

        result = response.json()
        code = result['content'][0]['text']
        watermarked_code = add_agent_watermark(code, "kimi", task_id, output_file)

        result_path.parent.mkdir(parents=True, exist_ok=True)
        with open(result_path, 'w', encoding='utf-8') as f:
            f.write(watermarked_code)

        log_task_event(task_id, "SUCCESS", "SAVE", f"Code saved to {output_file}")
        update_task_status(task_id, "running", progress=70, step="File saved")

        validation = run_validation(task_id, spec.get('validation_command'))
        status = "completed" if validation['passed'] else "failed"

        conn = sqlite3.connect(DB_PATH)
        cursor = conn.cursor()
        cursor.execute("""
            UPDATE tasks
            SET status = ?, validation_report = ?, result_path = ?, completed_at = CURRENT_TIMESTAMP
            WHERE id = ?
        """, (status, json.dumps(validation), str(result_path), task_id))
        conn.commit()
        conn.close()

        log_task_event(task_id, "SUCCESS" if validation['passed'] else "WARNING",
                       "COMPLETE", f"Task completed with status: {status}")
        update_task_status(task_id, status, progress=100, step="Complete")

    except Exception as e:
        log_task_event(task_id, "ERROR", "GENERATION", str(e))
        update_task_status(task_id, "failed", error=str(e))
        raise

def run_validation(task_id: str, validation_command: Optional[str]) -> Dict[str, Any]:
    if not validation_command:
        return {"passed": True, "message": "No validation command specified"}

    try:
        result = subprocess.run(
            validation_command,
            shell=True,
            capture_output=True,
            text=True,
            timeout=120
        )

        passed = result.returncode == 0
        return {
            "passed": passed,
            "stdout": result.stdout[:2000],  # Limit output size
            "stderr": result.stderr[:2000],
            "returncode": result.returncode
        }
    except Exception as e:
        return {"passed": False, "error": str(e)}

# API Endpoints
@app.post("/task/create", response_model=TaskCreateResponse)
def create_task(spec: TaskSpec, background_tasks: BackgroundTasks = None):
    """Create new task and optionally start in background"""
    task_id = generate_task_id()
    spec_path = save_spec(task_id, spec)

    conn = sqlite3.connect(DB_PATH)
    cursor = conn.cursor()
    cursor.execute("""
        INSERT INTO tasks (id, agent_type, status, spec_path, progress_percent)
        VALUES (?, ?, 'pending', ?, 0)
    """, (task_id, spec.agent_type, spec_path))
    conn.commit()
    conn.close()

    log_task_event(task_id, "INFO", "CREATE", f"Task created for agent {spec.agent_type}")

    return TaskCreateResponse(
        task_id=task_id,
        status="pending",
        message=f"Task created. Start with: POST /task/{task_id}/delegate"
    )

@app.post("/task/{task_id}/delegate")
async def delegate_task(task_id: str, background: bool = True, background_tasks: BackgroundTasks = None):
    """Start task execution. If background=true, runs asynchronously"""
    conn = sqlite3.connect(DB_PATH)
    cursor = conn.cursor()
    cursor.execute("SELECT id, agent_type, spec_path FROM tasks WHERE id = ?", (task_id,))
    row = cursor.fetchone()
    conn.close()

    if not row:
        raise HTTPException(status_code=404, detail="Task not found")

    _, agent_type, spec_path = row

    if background:
        # Start in background using threading (more reliable than asyncio.create_task)
        import threading
        thread = threading.Thread(
            target=lambda: asyncio.run(run_agent_background(task_id, spec_path, agent_type)),
            daemon=True
        )
        thread.start()
        return {
            "task_id": task_id,
            "status": "running",
            "mode": "background",
            "message": f"Task started in background. Check status at /task/{task_id}/status"
        }
    else:
        # Synchronous execution (for testing)
        return {"task_id": task_id, "status": "sync_mode", "message": "Use background=true for production"}

@app.get("/task/{task_id}/status", response_model=TaskStatusResponse)
def get_status(task_id: str):
    """Get current task status with progress"""
    conn = sqlite3.connect(DB_PATH)
    cursor = conn.cursor()
    cursor.execute("""
        SELECT id, agent_type, status, progress_percent, current_step,
               validation_report, error, started_at, completed_at
        FROM tasks WHERE id = ?
    """, (task_id,))
    row = cursor.fetchone()
    conn.close()

    if not row:
        raise HTTPException(status_code=404, detail="Task not found")

    return TaskStatusResponse(
        task_id=row[0],
        agent_type=row[1],
        status=row[2],
        progress_percent=row[3] or 0,
        current_step=row[4],
        validation_report=json.loads(row[5]) if row[5] else None,
        error=row[6],
        started_at=row[7],
        completed_at=row[8]
    )

@app.get("/task/{task_id}/report", response_model=TaskReportResponse)
def get_report(task_id: str):
    """Get detailed task report with logs, watermarks, and validation"""
    conn = sqlite3.connect(DB_PATH)
    cursor = conn.cursor()

    # Get task info
    cursor.execute("""
        SELECT id, agent_type, status, result_path, validation_report, started_at, completed_at
        FROM tasks WHERE id = ?
    """, (task_id,))
    task_row = cursor.fetchone()

    if not task_row:
        conn.close()
        raise HTTPException(status_code=404, detail="Task not found")

    # Get logs
    cursor.execute("""
        SELECT timestamp, level, step, message
        FROM task_logs WHERE task_id = ? ORDER BY timestamp
    """, (task_id,))
    logs = [{"timestamp": r[0], "level": r[1], "step": r[2], "message": r[3]} for r in cursor.fetchall()]

    conn.close()

    # Calculate time elapsed
    time_elapsed = 0
    if task_row[5]:  # started_at
        if task_row[6]:  # completed_at
            time_elapsed = int((datetime.fromisoformat(task_row[6]) - datetime.fromisoformat(task_row[5])).total_seconds())
        else:
            time_elapsed = int((datetime.utcnow() - datetime.fromisoformat(task_row[5])).total_seconds())

    # Verify watermarks on generated files
    watermarks = []
    files_generated = []
    if task_row[3]:  # result_path
        result_path = Path(task_row[3])
        if result_path.exists():
            files_generated.append(str(result_path))
            wm = verify_agent_watermark(result_path)
            wm["file"] = str(result_path)
            watermarks.append(wm)

    # Generate summary
    summary = generate_report_summary(task_id, task_row[1], task_row[2], logs)

    return TaskReportResponse(
        task_id=task_row[0],
        agent_type=task_row[1],
        status=task_row[2],
        summary=summary,
        detailed_log=logs,
        files_generated=files_generated,
        validation_results=json.loads(task_row[4]) if task_row[4] else None,
        agent_watermarks=watermarks,
        time_elapsed_seconds=time_elapsed
    )

def generate_report_summary(task_id: str, agent_type: str, status: str, logs: List[Dict]) -> str:
    """Generate human-readable summary from logs"""
    if not logs:
        return f"Task {task_id} ({agent_type}): {status}"

    steps = []
    errors = []
    success_count = sum(1 for log in logs if log["level"] == "SUCCESS")

    for log in logs:
        if log["level"] == "ERROR":
            errors.append(log["message"])
        elif log["step"] not in [s["step"] for s in steps]:
            steps.append(log)

    summary = f"""## Task Report: {task_id}
**Agent:** {agent_type}
**Status:** {status}
**Steps completed:** {len(steps)}
"""

    if errors:
        summary += f"\n**Errors encountered:** {len(errors)}\n"
        for err in errors[:3]:
            summary += f"- {err[:100]}\n"

    summary += "\n**Progress:**\n"
    for step in steps[-5:]:  # Last 5 steps
        icon = "✅" if step["level"] == "SUCCESS" else "🔄" if step["level"] == "INFO" else "⚠️"
        summary += f"{icon} {step['step']}: {step['message'][:80]}\n"

    return summary

@app.get("/task/{task_id}/verify")
def verify_task_files(task_id: str):
    """Verify all files generated by this task have valid watermarks"""
    conn = sqlite3.connect(DB_PATH)
    cursor = conn.cursor()
    cursor.execute("SELECT result_path FROM tasks WHERE id = ?", (task_id,))
    row = cursor.fetchone()
    conn.close()

    if not row or not row[0]:
        raise HTTPException(status_code=404, detail="Task or result not found")

    result_path = Path(row[0])
    verification = verify_agent_watermark(result_path)

    return {
        "task_id": task_id,
        "file": str(result_path),
        "verification": verification,
        "sonnet_safe": verification.get("verified", False)
    }

@app.get("/tasks")
def list_tasks(limit: int = 20, status: str = None):
    """List recent tasks with optional status filter"""
    conn = sqlite3.connect(DB_PATH)
    cursor = conn.cursor()

    if status:
        cursor.execute("""
            SELECT id, agent_type, status, progress_percent, current_step, created_at
            FROM tasks WHERE status = ? ORDER BY created_at DESC LIMIT ?
        """, (status, limit))
    else:
        cursor.execute("""
            SELECT id, agent_type, status, progress_percent, current_step, created_at
            FROM tasks ORDER BY created_at DESC LIMIT ?
        """, (limit,))

    rows = cursor.fetchall()
    conn.close()

    return [
        {
            "id": row[0],
            "agent_type": row[1],
            "status": row[2],
            "progress_percent": row[3],
            "current_step": row[4],
            "created_at": row[5]
        }
        for row in rows
    ]

@app.get("/health")
def health():
    return {
        "status": "ok",
        "service": "Task Orchestration Daemon v2",
        "features": ["background_tasks", "watermarking", "detailed_reports"]
    }

@app.get("/")
def root():
    return {
        "service": "Task Orchestration Daemon v2",
        "version": "2.0.0",
        "endpoints": {
            "POST /task/create": "Create new task (with optional background mode)",
            "POST /task/{id}/delegate?background=true": "Start task in background",
            "GET /task/{id}/status": "Get task status with progress",
            "GET /task/{id}/report": "Get detailed task report",
            "GET /task/{id}/verify": "Verify agent watermarks",
            "GET /tasks": "List recent tasks",
            "GET /health": "Health check"
        },
        "agents": ["glm", "kimi"],
        "protections": ["agent_watermarks", "git_hooks", "verification_endpoint"]
    }

if __name__ == "__main__":
    import uvicorn
    init_db()
    logger.info("Starting Orchestrator v2 with background task support")
    uvicorn.run(app, host="127.0.0.1", port=8000)
