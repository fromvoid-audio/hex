// Controlled lifecycle negative control, never loads a plugin itself.
#include <cerrno>
#include <csignal>
#include <cstdio>
#include <cstring>
#include <unistd.h>
#include <sys/wait.h>
int main(int argc,char** argv) {
 if(argc<2)return 2;
 struct sigaction ignored{};ignored.sa_handler=SIG_IGN;ignored.sa_flags=SA_NOCLDWAIT;
 if(sigaction(SIGCHLD,&ignored,nullptr)!=0)return 3;
 if(std::strcmp(argv[1],"--negative-control")==0){
  auto pid=fork();if(pid<0)return 4;if(pid==0)_exit(0);
  int status=0;if(waitpid(pid,&status,0)!=-1||errno!=ECHILD)return 5;
  std::puts("inherited auto-reap negative control confirmed");return 0;
 }
 execv(argv[1],argv+1);return 6;
}
