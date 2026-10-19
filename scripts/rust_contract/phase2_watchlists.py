"""Watchlist route schedules registered with the one shared runner."""
from datetime import UTC, datetime, timedelta

from scripts.rust_contract.phase2_scenarios import (
    Action,
    Scenario,
    ScenarioError,
    Server,
)

OWNER="20000000-0000-4000-8000-000000000001"
CONTAINER="70000000-0000-4000-8000-000000000001"


def watchlist_reads_scenario():
    def setup(connection):
        connection.execute("INSERT INTO watchlists(id,user_id,name,kind,created_at) VALUES(%s,%s,'Read fixture','CUSTOM','2026-10-19T00:00:00Z')",(CONTAINER,OWNER))
    def servers(context):
        return [Server((context.python_bin,"-m","scripts.rust_contract.phase2_oracle","--port","{port}")),
            Server((str(context.root/"rs/target/debug/examples/watchlist_server"),),{"WATCHLIST_TEST_CUTOFF":"2026-10-19T00:00:00Z"})]
    def unchanged(before,after,response,mail):
        if before!=after or mail:
            raise ScenarioError("watchlist read changed state or sent email")
    def actions(context):
        import jwt
        now=datetime.now(UTC)
        token=jwt.encode({"sub":OWNER,"type":"access","iss":"verdaxis-test-api","aud":"verdaxis-test-web",
                          "iat":int(now.timestamp()),"exp":int((now+timedelta(minutes=30)).timestamp())},context.environment["JWT_SECRET"],algorithm="HS256")
        headers={"authorization":"Bearer "+token,"x-request-id":"watchlist-read-case"}
        return [Action("legacy-list","W01","GET","/api/watchlists",200,headers=headers,assert_after=unchanged),
            Action("own-detail","W04","GET","/api/watchlists/"+CONTAINER,200,headers=headers,assert_after=unchanged),
            Action("missing-detail","W04","GET","/api/watchlists/70000000-0000-4000-8000-000000000099",404,headers=headers,assert_after=unchanged),
            Action("bad-bearer-before-uuid","W04","GET","/api/watchlists/not-a-uuid",401,headers={"authorization":"Bearer invalid","x-request-id":"watchlist-read-case"},assert_after=unchanged)]
    return Scenario("watchlist-http-reads","watchlists",setup,servers,actions,
        build=("cargo","build","--manifest-path","rs/Cargo.toml","--locked","-p","verdaxis-read","--features","contract-tests","--example","watchlist_server"),
        route_qualification=False)
