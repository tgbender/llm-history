"""Retrieval evaluation with the pinned local models over the frozen 40-question corpus.

All subprocesses are bounded and cleaned up. Uses synthetic text only. Run with
the Python reference venv for the optional cached MLX numerical comparison.
"""
import argparse
import contextlib
import json
import math
import os
from pathlib import Path
import secrets
import socket
import subprocess
import time
import urllib.request

ROOT=Path(__file__).resolve().parents[1]
from artifacts import CACHE
OUT=ROOT/".cache/evaluation"


def model_path(name):
    from huggingface_hub import try_to_load_from_cache
    artifact=next(a for a in json.loads((ROOT/"artifacts.json").read_text())["models"] if a["name"]==name)
    cached=try_to_load_from_cache(artifact["repository"],artifact.get("filename",name),revision=artifact["revision"])
    if isinstance(cached,str):return Path(cached)
    raise FileNotFoundError(f"Model missing from the Hugging Face cache: {name}; run models fetch --download-model")


def request(endpoint,path,payload=None,key=None):
    headers={"Authorization":"Bearer "+key} if key else {}
    if payload is not None: headers["Content-Type"]="application/json"
    req=urllib.request.Request(endpoint+path,data=None if payload is None else json.dumps(payload).encode(),headers=headers)
    with urllib.request.urlopen(req,timeout=90) as response:return json.load(response)


@contextlib.contextmanager
def server(model,rerank,key):
    with socket.socket() as sock:sock.bind(("127.0.0.1",0));port=sock.getsockname()[1]
    endpoint=f"http://127.0.0.1:{port}"
    log=(OUT/("reranker.log" if rerank else "embedder.log")).open("w")
    args=[str(CACHE/"llama-server"),"--host","127.0.0.1","--port",str(port),"--api-key",key,"--model",str(model_path(model)),"--ctx-size","8192","--parallel","1","--batch-size","8192","--ubatch-size","512","--no-webui"]
    args += ["--reranking","--pooling","rank"] if rerank else ["--embeddings","--pooling","mean"]
    process=subprocess.Popen(args,stdin=subprocess.DEVNULL,stdout=log,stderr=log)
    try:
        deadline=time.monotonic()+90
        while time.monotonic()<deadline:
            if process.poll() is not None:raise RuntimeError(f"model startup failed: {log.name}")
            try:request(endpoint,"/health",key=key);break
            except Exception:time.sleep(0.1)
        else:raise TimeoutError("model startup")
        yield endpoint
    finally:
        process.terminate()
        try:process.wait(timeout=10)
        except subprocess.TimeoutExpired:process.kill();process.wait(timeout=10)
        log.close()


def cosine(a,b):
    return sum(x*y for x,y in zip(a,b))/(sum(x*x for x in a)*sum(x*x for x in b))**0.5


def main():
    global OUT
    parser=argparse.ArgumentParser()
    parser.add_argument("--mlx",action="store_true")
    parser.add_argument("--smoke-only",action="store_true")
    parser.add_argument("--binary",type=Path,default=ROOT/"target/debug/llm-history")
    parser.add_argument("--output",type=Path,default=OUT)
    args=parser.parse_args()
    OUT=args.output
    OUT.mkdir(parents=True,exist_ok=True);OUT.chmod(0o700)
    corpus=json.loads((ROOT/"fixtures/retrieval.json").read_text())
    binary=args.binary;db=OUT/"evaluation.duckdb"
    if db.exists():raise SystemExit("evaluation database exists; use a fresh evaluation output directory")
    env=dict(os.environ,LLM_HISTORY_RS_BACKGROUND="off",LLM_HISTORY_BACKGROUND="off",HF_HUB_OFFLINE="1")
    def run(*arguments,timeout=180):
        result=subprocess.run([str(binary),"--db",str(db),"--cache",str(CACHE),"--json",*map(str,arguments)],env=env,text=True,capture_output=True,timeout=timeout)
        assert result.returncode==0,result.stderr
        return json.loads(result.stdout)
    profiles={}
    lookup={}
    for doc in corpus["documents"]:
        profile=doc["profile"]+"-"+doc["harness"]
        if profile not in profiles:
            root=OUT/profile;folder=root/("sessions" if doc["harness"]=="codex" else "projects/project");folder.mkdir(parents=True,exist_ok=True)
            run("profile","add",profile,"--harness",doc["harness"],"--root",root);profiles[profile]=folder
        if doc["harness"]=="codex":
            records=[{"type":"session_meta","timestamp":doc["timestamp"],"payload":{"id":doc["native_session_id"],"cwd":doc["cwd"]}},{"type":"response_item","timestamp":doc["timestamp"],"payload":{"type":"message","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":doc["text"]}]}}]
            native=doc["native_session_id"]
        else:
            records=[{"type":"assistant","timestamp":doc["timestamp"],"sessionId":doc["native_session_id"],"cwd":doc["cwd"],"message":{"id":doc["id"],"role":"assistant","stop_reason":"end_turn","content":[{"type":"text","text":doc["text"]}]}}]
            native="claude:"+doc["native_session_id"]
        (profiles[profile]/(doc["id"]+".jsonl")).write_text("".join(json.dumps(r)+"\n" for r in records))
        lookup[(profile,native)]=doc["id"]
    run("update","--no-background")
    start=time.monotonic();backfill=run("semantic","backfill","--foreground","--seconds","120",timeout=240)
    assert backfill["status"]=="caught_up",backfill
    print("Managed Rust backfill complete:",backfill,flush=True)
    status=run("semantic","status");assert status["current"]==len(corpus["documents"]),status
    # Exercise Rust's own managed embedding and reranking process lifecycle.
    smoke=run("search","recover interrupted indexing","--mode","hybrid","--rerank","--no-refresh","--no-background",timeout=240)
    assert smoke["results"]
    report={"backfill":backfill,"backfill_wall_seconds":time.monotonic()-start,"managed_smoke":True,"metrics":{},"queries":[]}
    key=secrets.token_hex(24);keyfile=OUT/"api-key";keyfile.write_text(key);keyfile.chmod(0o600)
    try:
        with server("embeddinggemma-2-BF16.gguf",False,key) as embedding,server("qwen3-reranker-0.6b-q8_0.gguf",True,key) as reranking:
            texts=["repair a stale history projection","Use the retained generation to recover the archived source.","Unicode 🦀 and 中文 identifiers"]
            prompts=["title: none | text: "+t for t in texts]
            response=request(embedding,"/v1/embeddings",{"input":prompts,"model":"local"},key)
            vectors=[row["embedding"] for row in sorted(response["data"],key=lambda r:r["index"])]
            assert all(len(v)==768 and all(math.isfinite(x) for x in v) for v in vectors)
            from tokenizers import Tokenizer
            tokenizer=Tokenizer.from_file(str(model_path("embeddinggemma-2-tokenizer.json")))
            token_parity=[]
            for prompt in prompts:
                observed=request(embedding,"/tokenize",{"content":prompt,"add_special":True},key)["tokens"]
                assert observed==tokenizer.encode(prompt,add_special_tokens=True).ids
                token_parity.append(len(observed))
            report["token_parity"]=token_parity
            if args.mlx:
                from llm_history.semantic import MLXEmbeddingGemma2Embedder,embeddinggemma2_profile
                reference=MLXEmbeddingGemma2Embedder(embeddinggemma2_profile(),local_files_only=True)
                original=[row.vector for row in reference.embed(texts)]
                similarities=[cosine(a,b) for a,b in zip(original,vectors)]
                report["mlx_cosine_similarity"]=similarities
                assert min(similarities)>=0.999,similarities
                print("MLX cosine similarities:",similarities,flush=True)
                del reference
            if args.smoke_only:
                report["status"]="smoke_passed";(OUT/"report.json").write_text(json.dumps(report,indent=2)+"\n");print("PASS: managed embedding/reranking and numerical smoke",flush=True);return
            for mode in ("lexical","semantic","hybrid"):
                for rerank in (False,True):
                    values=[]
                    for case in corpus["queries"]:
                        command=["search",case["query"],"--mode",mode,"--limit","10","--per-session","10","--no-refresh","--no-background","--endpoint",embedding,"--api-key-file",keyfile,"--rerank-endpoint",reranking,"--rerank-api-key-file",keyfile]
                        if rerank:command.append("--rerank")
                        if "profile" in case:
                            command += ["--profile",case["profile"]+"-"+case["source"],"--source",case["source"],"--since",case["since"],"--timezone","UTC"]
                        if "project" in case:command += ["--project",case["project"]]
                        started=time.monotonic();result=run(*command)
                        ranked=[lookup[(r["profile"],r["thread_id"])] for r in result["results"]]
                        grade=case["relevant"]
                        positions=[i+1 for i,item in enumerate(ranked) if item in grade]
                        recall=len(positions)/len(grade);rr=1/min(positions) if positions else 0
                        dcg=sum((2**grade.get(item,0)-1)/math.log2(i+2) for i,item in enumerate(ranked))
                        ideal=sum((2**g-1)/math.log2(i+2) for i,g in enumerate(sorted(grade.values(),reverse=True)[:10]))
                        values.append((recall,rr,dcg/ideal if ideal else 0))
                        report["queries"].append({"id":case["id"],"mode":mode,"rerank":rerank,"ranked":ranked,"seconds":time.monotonic()-started})
                    report["metrics"][mode+("+rerank" if rerank else "")]=dict(zip(("recall@10","mrr@10","ndcg@10"),[sum(v[i] for v in values)/len(values) for i in range(3)]))
                    (OUT/"report.json").write_text(json.dumps(report,indent=2)+"\n")
                    print(mode,rerank,report["metrics"][mode+("+rerank" if rerank else "")],flush=True)
    finally:keyfile.unlink(missing_ok=True)
    report["status"]="passed";(OUT/"report.json").write_text(json.dumps(report,indent=2)+"\n")
    print("PASS: local managed models, full coverage, tokenizer parity, numerical checks and 40-question retrieval evaluation",flush=True)


if __name__=="__main__":main()
